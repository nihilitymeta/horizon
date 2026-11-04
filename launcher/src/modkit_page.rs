//! The modkit's card on the Mods page (`horizon_launcher::modkit`): the Lua library Event Horizon's
//! mods need, with the version installed, the newest release, an Install or Update button and its
//! "Update automatically" switch.
use super::*;
use horizon_launcher::modkit::{self, State};
use std::sync::mpsc::{channel, Receiver, Sender};

enum Event {
    /// The newest release's tag, or why it isn't known.
    Newest(Result<String, String>),
    Installed(Result<String, String>),
}

pub(super) struct Modkit {
    state: State,
    newest: Option<Result<String, String>>,
    asked: bool,
    busy: bool,
    message: Option<Result<String, String>>,
    tx: Sender<Event>,
    rx: Receiver<Event>,
}

impl Modkit {
    pub(super) fn new() -> Self {
        let (tx, rx) = channel();
        Self {
            state: modkit::load(),
            newest: None,
            asked: false,
            busy: false,
            message: None,
            tx,
            rx,
        }
    }
}

impl Launcher {
    pub(super) fn modkit_tick(&mut self) -> bool {
        let mut changed = false;
        while let Ok(event) = self.modkit.rx.try_recv() {
            changed = true;
            match event {
                Event::Newest(found) => self.modkit.newest = Some(found),
                Event::Installed(result) => {
                    self.modkit.busy = false;
                    self.modkit.state = modkit::load();
                    self.modkit.message = Some(result.map(|v| {
                        trf("Installed the modkit {version}.", &[("version", &v)]).to_string()
                    }));
                }
            }
        }
        changed
    }

    fn modkit_ask_newest(&mut self) {
        if self.modkit.asked {
            return;
        }
        self.modkit.asked = true;
        let tx = self.modkit.tx.clone();
        std::thread::spawn(move || {
            let found = modkit::latest(&horizon_launcher::github_mods::api).map(|r| r.tag);
            let _ = tx.send(Event::Newest(found));
        });
    }

    fn modkit_install(&mut self) {
        if self.modkit.busy {
            return;
        }
        self.modkit.busy = true;
        self.modkit.message = None;
        let tx = self.modkit.tx.clone();
        std::thread::spawn(move || {
            let result = modkit::latest(&horizon_launcher::github_mods::api)
                .and_then(|release| modkit::install(&release));
            let _ = tx.send(Event::Installed(result));
        });
    }

    /// The card, for a Horizon instance's Mods tab.
    pub(super) fn modkit_card(&mut self, cx: &mut Context<Self>) -> Div {
        self.modkit_ask_newest();
        let t = self.theme;
        let for_event_horizon = horizon_launcher::modkit::plays_event_horizon(
            self.selected_instance().and_then(|i| i.version.as_deref()),
        );
        let installed = self.modkit.state.installed.clone();
        let newest = self.modkit.newest.clone();
        let current = match (&installed, &newest) {
            (Some(i), Some(Ok(tag))) => &i.tag == tag,
            _ => false,
        };
        let mut status = vec![match &installed {
            Some(i) => trf("Installed {version}", &[("version", &i.version)]).to_string(),
            None => tr("Not installed").to_string(),
        }];
        match &newest {
            Some(Ok(tag)) if !current => status.push(
                trf(
                    "newest {version}",
                    &[("version", tag.trim_start_matches('v'))],
                )
                .to_string(),
            ),
            Some(Ok(_)) => status.push(tr("Up to date").to_string()),
            Some(Err(e)) => status.push(e.clone()),
            None => status.push(tr("Checking…").to_string()),
        }
        let label = if installed.is_some() {
            "Update"
        } else {
            "Install"
        };
        let mut action = button("modkit-install", label, Kind::Secondary, &t);
        action = if self.modkit.busy || current || !matches!(newest, Some(Ok(_))) {
            disabled(action)
        } else {
            action.on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.modkit_install();
                cx.notify();
            }))
        };
        let auto = switch("modkit-auto", self.modkit.state.auto_update, &t).on_click(cx.listener(
            |this, _: &ClickEvent, _, cx| {
                let on = !this.modkit.state.auto_update;
                if let Err(e) = modkit::set_auto_update(on) {
                    this.modkit.message = Some(Err(e));
                }
                this.modkit.state = modkit::load();
                cx.notify();
            },
        ));
        let mut card = div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(caps(tr("Horizon Modkit · for Event Horizon")).text_color(t.muted))
            .child(text(theme::CAPTION).text_color(t.muted).child(if for_event_horizon {
                tr("The Lua library Event Horizon's mods are written with. It goes into every instance that plays an Event Horizon build, turned on.")
            } else {
                tr("For Event Horizon: this instance plays a build of Horizon's, which doesn't run the modkit. Pick an Event Horizon build for it on the Versions page.")
            }))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(12.))
                    .child(
                        div().flex_1().child(
                            text(theme::UI).text_color(t.ink).child(if self.modkit.busy {
                                tr("Installing…").to_string()
                            } else {
                                status.join(" · ")
                            }),
                        ),
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
                    .child(action),
            );
        match &self.modkit.message {
            Some(Ok(done)) => {
                card = card.child(
                    text(theme::CAPTION)
                        .text_color(t.ink_soft)
                        .child(done.clone()),
                )
            }
            Some(Err(e)) => card = card.child(notice(e.clone(), &t)),
            None => {}
        }
        card
    }
}
