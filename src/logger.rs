use std::sync::{Arc, Mutex};
use std::time::SystemTime;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

use crate::{AppWindow, LogEntryData};

#[derive(Clone, Debug)]
pub struct LogItem {
    pub timestamp: String,
    pub level: String,
    pub message: String,
}

#[derive(Clone)]
pub struct UiLogLayer {
    entries: Arc<Mutex<Vec<LogItem>>>,
    window: Arc<Mutex<Option<slint::Weak<AppWindow>>>>,
}

impl UiLogLayer {
    pub fn new() -> Self {
        Self {
            entries: Arc::new(Mutex::new(Vec::with_capacity(500))),
            window: Arc::new(Mutex::new(None)),
        }
    }

    pub fn set_window(&self, weak: slint::Weak<AppWindow>) {
        if let Ok(mut guard) = self.window.lock() {
            *guard = Some(weak.clone());
        }
        let entries_clone = self.entries.clone();
        let _ = weak.upgrade_in_event_loop(move |win| {
            if let Ok(list) = entries_clone.lock() {
                let ui_items: Vec<LogEntryData> = list
                    .iter()
                    .map(|it| LogEntryData {
                        timestamp: it.timestamp.clone().into(),
                        level: it.level.clone().into(),
                        message: it.message.clone().into(),
                    })
                    .collect();
                win.set_log_entries(slint::ModelRc::from(std::rc::Rc::new(
                    slint::VecModel::from(ui_items),
                )));
            }
        });
    }

    pub fn clear(&self) {
        if let Ok(mut guard) = self.entries.lock() {
            guard.clear();
        }
        if let Ok(guard) = self.window.lock() {
            if let Some(weak) = guard.as_ref() {
                let _ = weak.upgrade_in_event_loop(|win| {
                    win.set_log_entries(slint::ModelRc::from(std::rc::Rc::new(
                        slint::VecModel::from(Vec::<LogEntryData>::new()),
                    )));
                    win.set_has_error(false);
                    win.set_error_message("".into());
                });
            }
        }
    }
}

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for UiLogLayer {
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let metadata = event.metadata();
        let level = metadata.level().to_string();

        struct MessageVisitor(String);
        impl tracing::field::Visit for MessageVisitor {
            fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
                if field.name() == "message" {
                    self.0 = format!("{:?}", value);
                }
            }
            fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
                if field.name() == "message" {
                    self.0 = value.to_string();
                }
            }
        }

        let mut visitor = MessageVisitor(String::new());
        event.record(&mut visitor);

        let clean_msg = visitor.0.trim_matches('"').to_string();
        if clean_msg.is_empty() {
            return;
        }

        let duration = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default();
        let total_secs = duration.as_secs();
        let hours = (total_secs / 3600) % 24;
        let mins = (total_secs / 60) % 60;
        let secs = total_secs % 60;
        let time_str = format!("{:02}:{:02}:{:02}", hours, mins, secs);

        let item = LogItem {
            timestamp: time_str,
            level: level.clone(),
            message: clean_msg.clone(),
        };

        if let Ok(mut lock) = self.entries.lock() {
            if lock.len() >= 500 {
                lock.remove(0);
            }
            lock.push(item);
        }

        if let Ok(guard) = self.window.lock() {
            if let Some(weak) = guard.as_ref() {
                let is_error = level == "ERROR";
                let msg = clean_msg.clone();
                let entries_clone = self.entries.clone();

                let _ = weak.upgrade_in_event_loop(move |win| {
                    if is_error {
                        win.set_has_error(true);
                        win.set_error_message(msg.into());
                    }
                    if let Ok(list) = entries_clone.lock() {
                        let ui_items: Vec<LogEntryData> = list
                            .iter()
                            .map(|it| LogEntryData {
                                timestamp: it.timestamp.clone().into(),
                                level: it.level.clone().into(),
                                message: it.message.clone().into(),
                            })
                            .collect();
                        win.set_log_entries(slint::ModelRc::from(std::rc::Rc::new(
                            slint::VecModel::from(ui_items),
                        )));
                    }
                });
            }
        }
    }
}

pub fn init_subscribers(ui_layer: UiLogLayer) {
    let fmt_layer = tracing_subscriber::fmt::layer();
    let subscriber = tracing_subscriber::registry()
        .with(fmt_layer)
        .with(ui_layer);

    let _ = subscriber.try_init();
}
