//! The save screen freezes simulation, but keeps pumping window events while
//! snapshots are prepared in small batches and the region workers drain.
use super::{Game, loading_frame};
use horizon_client::{Result, hud::HudBatch};
use std::{
    collections::{HashSet, VecDeque},
    time::{Duration, Instant},
};

pub(super) struct ClosingSave {
    pub quit: bool,
    close: bool,
    reply: Option<std::sync::mpsc::Sender<String>>,
    started: Instant,
    /// How far the server-stage wait had got the last time it warned: its
    /// warnings stay at most `SERVER_STOP_WARN` apart.
    warned: Option<Duration>,
    /// Whether the wait passed `SERVER_STOP_TIMEOUT`: its slowness is an
    /// error in the log once, while the close keeps waiting for the save.
    server_timed_out: bool,
    stage: Stage,
}
enum Stage {
    Generation,
    Chunks(VecDeque<(i32, i32)>),
    Entities(VecDeque<(i32, i32)>),
    Writes,
    Server,
    Quit,
}

// Bound the work in both sparse and block-entity-heavy worlds. A single chunk
// is indivisible; check the time budget after each snapshot, not each batch.
const SNAPSHOTS_PER_FRAME: usize = 16;
const SNAPSHOT_BUDGET: Duration = Duration::from_millis(2);
// How long the closing save waits for the integrated server to stop before
// calling the wait slow: long enough for a genuinely slow final save on a
// big world. Past this point the lateness is an error in the log, but the
// wait goes on, so a slow save still finishes instead of being detached
// mid-write.
const SERVER_STOP_TIMEOUT: Duration = Duration::from_secs(60);
// How long the closing save waits before it gives up on the server: past
// this point a stuck server must not hold the "Saving world..." screen
// forever, so its thread is detached and the close fails loudly.
const SERVER_STOP_ABANDON: Duration = Duration::from_secs(600);
const SERVER_ABANDON_ERROR: &str =
    "the integrated server did not stop in time; its world may not be fully saved";

/// Whether the closing save gives up on an integrated server that has not
/// stopped after `waiting`: only past the abandon point, so a slow final
/// save still finishes. A stuck server never holds the screen past it.
fn server_stop_give_up(waiting: Duration) -> bool {
    waiting >= SERVER_STOP_ABANDON
}

/// What the closing save does with an integrated server still stopping.
#[derive(Debug, PartialEq)]
enum ServerStage {
    /// Keep waiting. `slow` is set on the one poll that first passes
    /// `SERVER_STOP_TIMEOUT`: the poll that logs the slow error.
    Wait { slow: bool },
    /// Detach the server and fail the close with this message.
    Abandon(&'static str),
}

/// The closing save's decision after waiting `waiting` for the server to
/// stop. `timed_out` is the one-shot flag: set once the slow wait is reported.
fn poll_server_stage(waiting: Duration, timed_out: &mut bool) -> ServerStage {
    if server_stop_give_up(waiting) {
        return ServerStage::Abandon(SERVER_ABANDON_ERROR);
    }
    if !*timed_out && waiting >= SERVER_STOP_TIMEOUT {
        *timed_out = true;
        return ServerStage::Wait { slow: true };
    }
    ServerStage::Wait { slow: false }
}
// How often the wait warns while the server is still stopping.
const SERVER_STOP_WARN: Duration = Duration::from_secs(10);

impl Game<'_> {
    pub(super) fn begin_closing_save(&mut self, quit: bool) -> Result<()> {
        if let Some(save) = &mut self.pending_save {
            save.quit |= quit;
            return Ok(());
        }
        if self.title.is_some() || self.saved_for_exit {
            // A quit from the title still needs the event loop to finish.
            self.pending_save = Some(ClosingSave {
                quit,
                close: true,
                reply: None,
                started: Instant::now(),
                warned: None,
                server_timed_out: false,
                stage: Stage::Quit,
            });
            return Ok(());
        }
        // Stop client-side terrain work before the integrated server is shut down. The
        // server's own stop path aborts its workers, but the window's streaming registration
        // is dropped only after the event loop exits and would otherwise wait for queued work.
        self.streaming.abort_generation();
        if let Some(integrated) = &self.integrated {
            integrated.request_stop();
            self.pending_save = Some(ClosingSave {
                quit,
                close: true,
                reply: None,
                started: Instant::now(),
                warned: None,
                server_timed_out: false,
                stage: Stage::Server,
            });
        } else if self.remote.is_some() {
            if !quit {
                return self.leave_server(None);
            }
            if let Some(mut remote) = self.remote.take() {
                remote.session.disconnect();
            }
            self.saved_for_exit = true;
            self.pending_save = Some(ClosingSave {
                quit,
                close: true,
                reply: None,
                started: Instant::now(),
                warned: None,
                server_timed_out: false,
                stage: Stage::Quit,
            });
        } else {
            self.close_container();
            self.award_custom_stat("leave_game", 1);
            self.lan_close(false);
            self.free_player_beds()?;
            self.pending_save = Some(ClosingSave {
                quit,
                close: true,
                reply: None,
                started: Instant::now(),
                warned: None,
                server_timed_out: false,
                stage: Stage::Generation,
            });
        }
        self.capture(false);
        Ok(())
    }

    /// Control saves acknowledge only after durable completion, while their
    /// snapshot work is spread across event-loop turns too.
    pub(super) fn begin_control_save(
        &mut self,
        all: bool,
        reply: std::sync::mpsc::Sender<String>,
    ) -> Result<()> {
        if self.pending_save.is_some() {
            return Err("a world save is already in progress".into());
        }
        if all {
            self.world
                .modified
                .extend(self.world.chunks.keys().copied());
        }
        self.keys.clear();
        self.physical_keys.clear();
        self.left = false;
        self.right = false;
        self.pending_save = Some(ClosingSave {
            quit: false,
            close: false,
            reply: Some(reply),
            started: Instant::now(),
            warned: None,
            server_timed_out: false,
            stage: Stage::Generation,
        });
        Ok(())
    }

    /// `true` means the window can exit. The session lock stays in Game until
    /// every snapshot has been queued and every writer has synced its files.
    pub(super) fn poll_closing_save(&mut self) -> Result<bool> {
        let Some(mut save) = self.pending_save.take() else {
            return Ok(false);
        };
        let result = (|| -> Result<bool> {
            match &mut save.stage {
                Stage::Quit => Ok(true),
                Stage::Server => {
                    let server = self
                        .integrated
                        .as_ref()
                        .ok_or("the integrated server stopped with an error")?;
                    if !server.stopped() {
                        let waiting = save.started.elapsed();
                        // A stuck server must not hold the closing save
                        // forever: the window detaches its thread instead.
                        // Past the first timeout the wait is officially slow:
                        // say so once, loudly, and keep waiting for the save
                        // instead of detaching its thread mid-write.
                        match poll_server_stage(waiting, &mut save.server_timed_out) {
                            ServerStage::Abandon(message) => {
                                log::error!(
                                    "Horizon: the integrated server did not stop in time \
                                     (running: {}, autosaving: {}, smoothed tick {:.1} ms); \
                                     its world may not be fully saved",
                                    !server.stopped(),
                                    server.is_saving(),
                                    server.smoothed_tick_ms()
                                );
                                self.integrated.take().unwrap().abandon();
                                return Err(message.into());
                            }
                            ServerStage::Wait { slow: true } => log::error!(
                                "Horizon: the integrated server did not stop within {} s \
                                 (running: {}, autosaving: {}, smoothed tick {:.1} ms); \
                                 the close keeps waiting for its world to save",
                                SERVER_STOP_TIMEOUT.as_secs(),
                                !server.stopped(),
                                server.is_saving(),
                                server.smoothed_tick_ms()
                            ),
                            ServerStage::Wait { slow: false } => {}
                        }
                        if save
                            .warned
                            .is_none_or(|warned| waiting - warned >= SERVER_STOP_WARN)
                        {
                            log::warn!(
                                "Horizon: the server has not stopped after {} s \
                                 (running: {}, autosaving: {}, smoothed tick {:.1} ms)",
                                waiting.as_secs(),
                                !server.stopped(),
                                server.is_saving(),
                                server.smoothed_tick_ms()
                            );
                            save.warned = Some(waiting);
                        }
                        return Ok(false);
                    }
                    // Drop/join only after completion; leave_server's teardown
                    // can then run without blocking on the server's writes.
                    if save.quit {
                        self.integrated.take().unwrap().stop()?;
                    } else {
                        self.leave_server(None)?;
                    }
                    Ok(true)
                }
                Stage::Generation => {
                    if save.started.elapsed() > Duration::from_secs(30) {
                        return Err("chunk generation did not finish before saving".into());
                    }
                    if self.streaming.poll_save(&mut self.world) {
                        save.stage = Stage::Chunks(self.world.modified.iter().copied().collect());
                    }
                    Ok(false)
                }
                Stage::Chunks(keys) => {
                    let started = Instant::now();
                    for _ in 0..SNAPSHOTS_PER_FRAME {
                        let Some(&key) = keys.front() else { break };
                        let chunks = self.with_entities(self.world.chunks.get(&key));
                        self.save.save_later(
                            &chunks,
                            self.pack.assets.registry,
                            world_biomes!(self),
                        )?;
                        self.world.modified.remove(&key);
                        keys.pop_front();
                        if started.elapsed() >= SNAPSHOT_BUDGET {
                            break;
                        }
                    }
                    if keys.is_empty() {
                        self.spawn_loaded_entities();
                        save.stage = Stage::Entities(self.world.chunks.keys().copied().collect());
                    }
                    Ok(false)
                }
                Stage::Entities(keys) => {
                    let started = Instant::now();
                    for _ in 0..SNAPSHOTS_PER_FRAME {
                        let Some(&key) = keys.front() else { break };
                        let chunks = self.entity_chunks(&HashSet::from([key]), false);
                        self.save.save_entities_later(chunks)?;
                        keys.pop_front();
                        if started.elapsed() >= SNAPSHOT_BUDGET {
                            break;
                        }
                    }
                    if keys.is_empty() {
                        self.prune_entity_memory();
                        self.save_raids();
                        self.save.save_attachments(&self.world.attachments)?;
                        self.save.save_forced_chunks(&self.streaming.forced)?;
                        self.save_game_metadata()?;
                        save.stage = Stage::Writes;
                    }
                    Ok(false)
                }
                Stage::Writes => {
                    let players = self.finish_player_autosaves(false)?;
                    Ok(self.save.writes_finished()? && players)
                }
            }
        })();
        match result {
            Ok(true) => {
                if let Some(reply) = save.reply.take() {
                    let _ = reply.send(serde_json::json!({ "ok": true }).to_string());
                }
                if save.quit && !save.close {
                    // A window close during a control save still has to close
                    // containers and save the resulting inventory changes.
                    self.begin_closing_save(true)?;
                    return Ok(false);
                }
                if save.quit {
                    self.saved_for_exit = true;
                    Ok(true)
                } else {
                    if save.close && self.title.is_none() {
                        self.finish_perf();
                        self.finish_close_world()?;
                    }
                    self.clock.reset();
                    self.previous = Instant::now();
                    Ok(false)
                }
            }
            other => {
                if let Err(error) = &other
                    && let Some(reply) = save.reply.take()
                {
                    let _ =
                        reply.send(serde_json::json!({ "error": error.to_string() }).to_string());
                }
                // Keep ownership (and the lock) on errors too. Native shutdown
                // sees the same durable-write failure instead of declaring success.
                self.pending_save = Some(save);
                other
            }
        }
    }

    pub(super) fn render_closing_save(&mut self) -> Result<()> {
        if matches!(
            self.pending_save.as_ref().map(|save| &save.stage),
            Some(Stage::Quit)
        ) {
            return Ok(());
        }
        // disconnectWithSavingScreen: a GenericMessageScreen, its title and
        // no navigation.
        horizon_client::widgets::begin_screen(|| {
            horizon_client::widgets::ScreenInfo::new(
                "saving",
                super::screen_info::tr("menu.savingLevel", &[]),
            )
            .without_navigation()
        });
        let Some(renderer) = &mut self.renderer else {
            return Ok(());
        };
        let mut hud = HudBatch::new([renderer.config.width, renderer.config.height]);
        let progress = match self.pending_save.as_ref().map(|save| &save.stage) {
            Some(Stage::Chunks(_)) => 0.25,
            Some(Stage::Entities(_)) => 0.5,
            Some(Stage::Writes) => 0.9,
            _ => 0.0,
        };
        horizon_client::level_loading::draw_progress(
            &mut hud,
            &self.gui.atlas,
            &super::screen_info::tr("menu.savingChunks", &[]),
            progress,
        );
        renderer.render(&loading_frame(&hud))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A slow final save still finishes past the first timeout; only a
    /// stuck server is detached, never silently and never after just a
    /// minute of writing.
    #[test]
    fn a_slow_server_keeps_its_wait_past_the_first_timeout() {
        assert!(SERVER_STOP_ABANDON > SERVER_STOP_TIMEOUT);
        assert!(!server_stop_give_up(Duration::from_secs(59)));
        assert!(
            !server_stop_give_up(Duration::from_secs(61)),
            "a slow final save still finishes after the first timeout"
        );
        assert!(
            server_stop_give_up(SERVER_STOP_ABANDON),
            "a stuck server never holds the saving screen"
        );
    }

    /// The closing save's server wait, through the decision it takes: 59 s
    /// keeps waiting silently, the first poll past 60 s reports the slow wait
    /// once and keeps waiting, and only 600 s abandons the server.
    #[test]
    fn poll_server_stage_warns_once_and_abandons_only_a_stuck_server() {
        let mut timed_out = false;
        assert_eq!(
            poll_server_stage(Duration::from_secs(59), &mut timed_out),
            ServerStage::Wait { slow: false }
        );
        assert!(!timed_out, "59 s is not yet the slow wait");
        assert_eq!(
            poll_server_stage(Duration::from_secs(61), &mut timed_out),
            ServerStage::Wait { slow: true },
            "the first poll past 60 s reports the slow wait"
        );
        assert!(timed_out, "the one-shot flag is set");
        assert_eq!(
            poll_server_stage(Duration::from_secs(62), &mut timed_out),
            ServerStage::Wait { slow: false },
            "the slow wait is reported once, not on every poll"
        );
        assert_eq!(
            poll_server_stage(SERVER_STOP_ABANDON, &mut timed_out),
            ServerStage::Abandon(
                "the integrated server did not stop in time; its world may not be fully saved"
            )
        );
    }
}
