pub trait Hooks: std::any::Any + Send + Sync {
#[doc(hidden)] fn subscribed(&self) -> &'static [Option<HookId>];
#[doc(hidden)] fn listeners(&self) -> &'static [Listener] { &[] }
fn register(&self, r: &mut dyn Registrar, cx: &RegistrationContext) { let _ = (r, cx); }
fn server_tick(&self, host: &mut dyn ServerHost, event: &mut ServerTickEvent<'_>) { let _ = host;let _ = event;  }
fn level_tick_pre(&self, host: HostSide, event: &mut LevelTickPreEvent) { let _ = host;let _ = event;  }
fn render_hand(&self, host: &dyn ClientHost, event: &mut RenderHandEvent<'_, '_>) -> Flow { let _ = host;let _ = event; Flow::Continue }
fn block_receive_game(&self, host: &mut dyn ServerHost, event_: &mut BlockReceiveGameEvent<'_>) -> Flow { let _ = host;let _ = event_; Flow::Continue }
fn block_step_on(&self, event: &mut BlockStepOnEvent<'_>) { let _ = event;  }
}
