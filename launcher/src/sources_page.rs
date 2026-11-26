//! "GitHub sources" on the Versions page, in both looks: repositories whose GitHub releases are game
//! builds, such as Event Horizon's (`horizon_launcher::game_sources`). Each row says what it
//! installed, has its "Update automatically" switch, an Update button and a Remove button; a field
//! adds another repository. Installs run on a thread and report back through a channel.
use super::line_edit::{self, Key, LineEdit};
use super::*;
use horizon_launcher::game_sources::{self, Source};
use std::sync::mpsc::{channel, Receiver, Sender};

pub(super) struct Sources {
    text: LineEdit,
    focus: FocusHandle,
    focused: bool,
    list: Vec<Source>,
    /// The repository being updated, while it is.
    busy: Option<String>,
    message: Option<Result<String, String>>,
    tx: Sender<Result<String, String>>,
    rx: Receiver<Result<String, String>>,
}

impl Sources {
    pub(super) fn new(cx: &mut App) -> Self {
        let (tx, rx) = channel();
        Self {
            text: LineEdit::new(String::new(), 200, 40),
            focus: cx.focus_handle().tab_index(0).tab_stop(true),
            focused: false,
            list: game_sources::load(),
            busy: None,
            message: None,
            tx,
            rx,
        }
    }

    pub(super) fn note_focus(&mut self, window: &Window) {
        self.focused = self.focus.is_focused(window);
    }
}

/// Whether `tag` is a release to install over `installed` (the tag installed last, if any).
fn is_new(installed: Option<&str>, tag: &str) -> bool {
    installed != Some(tag)
}

/// The update for `source`: its newest release, installed unless it is the one installed already.
/// The text says what happened.
fn update_source(source: Source) -> Result<String, String> {
    let newest = game_sources::latest(&source.repo, &horizon_launcher::github_mods::api)?;
    let installed = source.installed.as_ref().map(|i| i.tag.as_str());
    match newest {
        Some(release) if is_new(installed, &release.tag) => {
            let version = game_sources::install(&release, true)?;
            Ok(format!("{} → {version}", release.label()))
        }
        _ => Ok(tr("Up to date").to_string()),
    }
}

impl Launcher {
    /// What an update started from the card did: the list, the installed versions and the
    /// selection are read again.
    pub(super) fn sources_tick(&mut self) -> bool {
        let mut changed = false;
        while let Ok(result) = self.sources.rx.try_recv() {
            changed = true;
            self.sources.busy = None;
            self.sources.message = Some(result);
            self.sources.list = game_sources::load();
            self.installed = backend::installed();
            self.settings.selected = backend::Settings::load().selected;
        }
        changed
    }

    fn sources_add(&mut self) {
        let text = self.sources.text.text().trim().to_owned();
        if text.is_empty() {
            return;
        }
        let message = game_sources::add(&text, true).map(|(repo, took_over)| {
            if took_over {
                // The disk's settings changed; the window's copy must too, or its next save
                // undoes the step-aside.
                game_sources::step_aside_for_horizon_updates(&mut self.settings);
                trf(
                    "Added {repo}. It updates itself, and Event Horizon's game updates are off.",
                    &[("repo", &repo)],
                )
                .to_string()
            } else {
                trf("Added {repo}.", &[("repo", &repo)]).to_string()
            }
        });
        let added = message.is_ok();
        self.sources.message = Some(message);
        if added {
            self.sources.text = LineEdit::new(String::new(), 200, 40);
        }
        self.sources.list = game_sources::load();
    }

    /// Installs `repo`'s newest release now, on a thread.
    fn sources_update(&mut self, repo: String) {
        if self.sources.busy.is_some() {
            return;
        }
        self.sources.busy = Some(repo.clone());
        self.sources.message = None;
        let source = self
            .sources
            .list
            .iter()
            .find(|s| game_sources::same_repo(&s.repo, &repo))
            .cloned();
        let tx = self.sources.tx.clone();
        std::thread::spawn(move || {
            let result = match source {
                Some(source) => update_source(source),
                None => Err(trf("{repo} is not a source.", &[("repo", &repo)]).to_string()),
            };
            let _ = tx.send(result);
        });
    }

    fn sources_field(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let t = self.theme;
        let focused = self.sources.focused;
        let field = div()
            .id("sources-link")
            .track_focus(&self.sources.focus)
            .flex()
            .items_center()
            .w(px(320.))
            .h(px(theme::FIELD + 4.))
            .px(px(12.))
            .overflow_hidden()
            .border_2()
            .border_color(if focused { t.accent } else { t.outline })
            .bg(t.well)
            .text_size(px(theme::UI.px))
            .cursor_text()
            .child(line_edit::content(
                &self.sources.text,
                &t,
                focused,
                false,
                false,
                "owner/repo",
            ))
            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                window.focus(&this.sources.focus, cx);
                cx.notify();
            }))
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                match line_edit::handle(&mut this.sources.text, ev, cx) {
                    Key::Enter => this.sources_add(),
                    Key::Escape => window.blur(cx),
                    Key::Pass => return,
                    _ => {}
                }
                cx.stop_propagation();
                cx.notify();
            }));
        line_edit::mouse(field, cx, |this| &mut this.sources.text)
    }

    /// The card: each source with what it installed, its switch, Update and Remove, then the field
    /// that adds one. While an update runs, every control on the card is inert.
    pub(super) fn sources_card(&self, cx: &mut Context<Self>) -> Div {
        let t = self.theme;
        let busy_any = self.sources.busy.is_some();
        let mut card = div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .mb(px(24.))
            .child(caps(tr("GitHub sources")).text_color(t.muted))
            .child(text(theme::CAPTION).text_color(t.muted).child(tr(
                "Game builds a repository publishes as GitHub releases, such as Event Horizon's. Each installs on an installed version's game files.",
            )));
        for (i, source) in self.sources.list.clone().into_iter().enumerate() {
            let busy = self.sources.busy.as_deref() == Some(source.repo.as_str());
            let installed = match &source.installed {
                _ if busy => tr("Installing…").to_string(),
                Some(done) => trf("Installed {tag}", &[("tag", &done.tag)]).to_string(),
                None => tr("Nothing installed yet").to_string(),
            };
            let repo = source.repo.clone();
            let mut auto = switch(format!("source-auto-{i}"), source.auto_update, &t);
            auto = match busy_any {
                true => disabled(auto),
                false => auto.on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    let on = this
                        .sources
                        .list
                        .iter()
                        .find(|s| game_sources::same_repo(&s.repo, &repo))
                        .is_some_and(|s| !s.auto_update);
                    if let Err(e) = game_sources::set_auto_update(&repo, on) {
                        this.sources.message = Some(Err(e));
                    }
                    this.sources.list = game_sources::load();
                    cx.notify();
                })),
            };
            let repo = source.repo.clone();
            let mut update = button(format!("source-update-{i}"), "Update", Kind::Secondary, &t);
            update = match busy_any {
                true => disabled(update),
                false => update.on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.sources_update(repo.clone());
                    cx.notify();
                })),
            };
            let repo = source.repo.clone();
            let mut remove = button(format!("source-remove-{i}"), "Remove", Kind::Secondary, &t);
            remove = match busy_any {
                true => disabled(remove),
                false => remove.on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    // The versions it installed stay; only the source is forgotten.
                    if let Err(e) = game_sources::remove(&repo) {
                        this.sources.message = Some(Err(e));
                    }
                    this.sources.list = game_sources::load();
                    cx.notify();
                })),
            };
            card = card.child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(12.))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .child(text(theme::UI).text_color(t.ink).child(source.repo.clone()))
                            .child(text(theme::CAPTION).text_color(t.muted).child(installed)),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(
                                text(theme::CAPTION)
                                    .text_color(t.muted)
                                    .child(tr("Update automatically")),
                            )
                            .child(auto),
                    )
                    .child(update)
                    .child(remove),
            );
        }
        let add = button("source-add", "Add", Kind::Secondary, &t);
        let add = match busy_any {
            true => disabled(add),
            false => add.on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.sources_add();
                cx.notify();
            })),
        };
        card = card.child(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(12.))
                .child(self.sources_field(cx))
                .child(add),
        );
        match &self.sources.message {
            Some(Ok(done)) => {
                card.child(plate(&t).p(px(12.)).text_color(t.ink).child(done.clone()))
            }
            Some(Err(e)) => card.child(notice(e.clone(), &t)),
            None => card,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::is_new;

    #[test]
    fn a_release_is_new_unless_it_is_the_one_installed() {
        assert!(is_new(None, "v1"));
        assert!(is_new(Some("v1"), "v2"));
        assert!(!is_new(Some("v2"), "v2"));
    }
}
