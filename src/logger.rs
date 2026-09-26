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
    last_dispatch: Arc<Mutex<std::time::Instant>>,
    filter_level: Arc<Mutex<i32>>,
    search_query: Arc<Mutex<String>>,
}

impl UiLogLayer {
    pub fn new() -> Self {
        Self {
            entries: Arc::new(Mutex::new(Vec::with_capacity(500))),
            window: Arc::new(Mutex::new(None)),
            last_dispatch: Arc::new(Mutex::new(std::time::Instant::now())),
            filter_level: Arc::new(Mutex::new(0)),
            search_query: Arc::new(Mutex::new(String::new())),
        }
    }

    pub fn set_window(&self, weak: slint::Weak<AppWindow>) {
        if let Ok(mut guard) = self.window.lock() {
            *guard = Some(weak);
        }
        self.dispatch_update();
    }

    pub fn set_filter(&self, level_idx: i32) {
        if let Ok(mut guard) = self.filter_level.lock() {
            *guard = level_idx;
        }
        self.dispatch_update();
    }

    pub fn set_search(&self, query: String) {
        if let Ok(mut guard) = self.search_query.lock() {
            *guard = query;
        }
        self.dispatch_update();
    }

    pub fn get_formatted_logs(&self) -> String {
        if let Ok(list) = self.entries.lock() {
            let filter_level = *self.filter_level.lock().unwrap_or_else(|e| e.into_inner());
            let query = self.search_query.lock().unwrap_or_else(|e| e.into_inner()).to_lowercase();

            let lines: Vec<String> = list
                .iter()
                .filter(|it| {
                    let matches_level = match filter_level {
                        1 => it.level == "INFO",
                        2 => it.level == "WARN",
                        3 => it.level == "ERROR",
                        _ => true,
                    };
                    if !matches_level {
                        return false;
                    }
                    if !query.is_empty() {
                        return it.message.to_lowercase().contains(&query)
                            || it.timestamp.contains(&query)
                            || it.level.to_lowercase().contains(&query);
                    }
                    true
                })
                .map(|it| format!("[{}] [{}] {}", it.timestamp, it.level, it.message))
                .collect();
            lines.join("\n")
        } else {
            String::new()
        }
    }

    pub fn dispatch_update(&self) {
        if let Ok(guard) = self.window.lock() {
            if let Some(weak) = guard.as_ref() {
                let entries_clone = self.entries.clone();
                let filter_level = *self.filter_level.lock().unwrap_or_else(|e| e.into_inner());
                let query = self.search_query.lock().unwrap_or_else(|e| e.into_inner()).to_lowercase();

                let _ = weak.upgrade_in_event_loop(move |win| {
                    if let Ok(list) = entries_clone.lock() {
                        let total_count = list.len() as i32;
                        let mut info_count = 0;
                        let mut warn_count = 0;
                        let mut error_count = 0;

                        for it in list.iter() {
                            match it.level.as_str() {
                                "ERROR" => error_count += 1,
                                "WARN" => warn_count += 1,
                                _ => info_count += 1,
                            }
                        }

                        win.set_log_total_count(total_count);
                        win.set_log_info_count(info_count);
                        win.set_log_warn_count(warn_count);
                        win.set_log_error_count(error_count);

                        let filtered: Vec<LogEntryData> = list
                            .iter()
                            .filter(|it| {
                                let matches_level = match filter_level {
                                    1 => it.level == "INFO",
                                    2 => it.level == "WARN",
                                    3 => it.level == "ERROR",
                                    _ => true,
                                };
                                if !matches_level {
                                    return false;
                                }
                                if !query.is_empty() {
                                    return it.message.to_lowercase().contains(&query)
                                        || it.timestamp.contains(&query)
                                        || it.level.to_lowercase().contains(&query);
                                }
                                true
                            })
                            .map(|it| LogEntryData {
                                timestamp: it.timestamp.clone().into(),
                                level: it.level.clone().into(),
                                message: it.message.clone().into(),
                            })
                            .collect();

                        win.set_log_entries(slint::ModelRc::from(std::rc::Rc::new(
                            slint::VecModel::from(filtered),
                        )));
                    }
                });
            }
        }
    }

    pub fn clear(&self) {
        if let Ok(mut guard) = self.entries.lock() {
            guard.clear();
        }
        self.dispatch_update();
    }
}

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for UiLogLayer {
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let metadata = event.metadata();
        let target = metadata.target();

        // Strictly ignore high-volume GUI/network/internal crates
        if !target.starts_with("native_video_downloader") {
            return;
        }

        if *metadata.level() > tracing::Level::INFO {
            return;
        }

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

        let is_error = level == "ERROR";
        let should_dispatch = if is_error {
            true
        } else if let Ok(mut last) = self.last_dispatch.lock() {
            if last.elapsed() >= std::time::Duration::from_millis(150) {
                *last = std::time::Instant::now();
                true
            } else {
                false
            }
        } else {
            false
        };

        if should_dispatch {
            self.dispatch_update();
        }
    }
}

pub fn init_subscribers(ui_layer: UiLogLayer) {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| {
            tracing_subscriber::EnvFilter::new("warn,native_video_downloader=info")
        });

    let fmt_layer = tracing_subscriber::fmt::layer();
    let subscriber = tracing_subscriber::registry()
        .with(filter)
        .with(fmt_layer)
        .with(ui_layer);

    let _ = subscriber.try_init();
}
