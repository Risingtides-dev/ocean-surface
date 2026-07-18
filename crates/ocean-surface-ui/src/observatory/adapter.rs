use futures_util::StreamExt;
use gloo_net::eventsource::futures::EventSource;
use gloo_net::http::Request;
use leptos::prelude::*;
use wasm_bindgen_futures::spawn_local;

use super::domain::{EventEnvelope, IntegrityState, ObservatorySnapshot, ObservatoryState};
use super::reducer::{apply, from_snapshot_preserving_slots};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ConnectionState {
    #[default]
    Connecting,
    Live,
    Resyncing,
    Offline,
}

impl ConnectionState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Connecting => "connecting",
            Self::Live => "live",
            Self::Resyncing => "resyncing",
            Self::Offline => "offline",
        }
    }
}

#[derive(Clone, Copy)]
pub struct ObservatoryClient {
    pub state: RwSignal<ObservatoryState>,
    pub connection: RwSignal<ConnectionState>,
    pub loading: RwSignal<bool>,
    generation: RwSignal<u64>,
}

impl ObservatoryClient {
    pub fn new() -> Self {
        Self {
            state: RwSignal::new(ObservatoryState::default()),
            connection: RwSignal::new(ConnectionState::Connecting),
            loading: RwSignal::new(true),
            generation: RwSignal::new(0),
        }
    }

    pub fn connect(self, base_url: RwSignal<String>) {
        let generation = self
            .generation
            .try_update(|generation| {
                *generation = generation.wrapping_add(1);
                *generation
            })
            .unwrap_or(0);
        spawn_local(async move {
            loop {
                if self.generation.get_untracked() != generation {
                    break;
                }
                self.connection.set(ConnectionState::Connecting);
                let base = base_url.get_untracked();
                if let Err(error) = self.fetch_snapshot(&base, None).await {
                    self.fail(error);
                    gloo_timers::future::TimeoutFuture::new(2_000).await;
                    continue;
                }
                let cursor = self.state.get_untracked().cursor;
                let stream_url = endpoint(&base, &format!("/v1/observatory/events?after={cursor}"));
                let mut source = match EventSource::new(&stream_url) {
                    Ok(source) => source,
                    Err(error) => {
                        self.fail(format!("event stream unavailable: {error}"));
                        gloo_timers::future::TimeoutFuture::new(2_000).await;
                        continue;
                    }
                };
                let message = match source.subscribe("message") {
                    Ok(stream) => stream,
                    Err(error) => {
                        self.fail(format!("event stream unavailable: {error}"));
                        gloo_timers::future::TimeoutFuture::new(2_000).await;
                        continue;
                    }
                };
                let reset = match source.subscribe("reset") {
                    Ok(stream) => stream,
                    Err(error) => {
                        self.fail(format!("reset stream unavailable: {error}"));
                        gloo_timers::future::TimeoutFuture::new(2_000).await;
                        continue;
                    }
                };
                self.connection.set(ConnectionState::Live);
                self.loading.set(false);
                let mut stream = futures_util::stream::select(message, reset);
                let mut needs_snapshot = false;
                while let Some(frame) = stream.next().await {
                    if self.generation.get_untracked() != generation {
                        source.close();
                        return;
                    }
                    let Ok((event_name, message)) = frame else {
                        break;
                    };
                    if event_name == "reset" {
                        needs_snapshot = true;
                        break;
                    }
                    let Some(data) = message.data().as_string() else {
                        continue;
                    };
                    match serde_json::from_str::<EventEnvelope>(&data) {
                        Ok(event) => {
                            self.state.update(|state| {
                                *state = apply(state.clone(), event);
                            });
                            if matches!(
                                self.state.get_untracked().integrity,
                                IntegrityState::Gap | IntegrityState::Stale
                            ) {
                                needs_snapshot = true;
                                break;
                            }
                        }
                        Err(error) => {
                            log::warn!("unparseable Observatory frame: {error}");
                        }
                    }
                }
                source.close();
                if needs_snapshot {
                    self.connection.set(ConnectionState::Resyncing);
                    let _ = self.fetch_snapshot(&base, None).await;
                } else {
                    self.connection.set(ConnectionState::Offline);
                    self.state.update(|state| {
                        state.integrity = IntegrityState::Disconnected;
                    });
                }
                gloo_timers::future::TimeoutFuture::new(1_200).await;
            }
        });
    }

    pub fn stop(self) {
        self.generation
            .update(|generation| *generation = generation.wrapping_add(1));
    }

    pub fn refresh(self, base_url: RwSignal<String>) {
        let base = base_url.get_untracked();
        spawn_local(async move {
            self.connection.set(ConnectionState::Resyncing);
            if let Err(error) = self.fetch_snapshot(&base, None).await {
                self.fail(error);
            } else {
                self.connection.set(ConnectionState::Live);
            }
        });
    }

    pub fn snapshot_at(self, base_url: RwSignal<String>, cursor: u64) {
        let base = base_url.get_untracked();
        spawn_local(async move {
            self.loading.set(true);
            if let Err(error) = self.fetch_snapshot(&base, Some(cursor)).await {
                self.fail(error);
            }
        });
    }

    async fn fetch_snapshot(&self, base: &str, cursor: Option<u64>) -> Result<(), String> {
        self.loading.set(true);
        let path = cursor
            .map(|cursor| format!("/v1/observatory/snapshot?at={cursor}"))
            .unwrap_or_else(|| "/v1/observatory/snapshot".to_owned());
        let response = Request::get(&endpoint(base, &path))
            .send()
            .await
            .map_err(|error| format!("snapshot unavailable: {error}"))?;
        if !response.ok() {
            let status = response.status();
            let detail = response.text().await.unwrap_or_default();
            return Err(format!("snapshot unavailable ({status}): {detail}"));
        }
        let snapshot = response
            .json::<ObservatorySnapshot>()
            .await
            .map_err(|error| format!("snapshot invalid: {error}"))?;
        // Preserve the session-local slot registry across refreshes, resyncs,
        // and replay scrubbing so existing cubicles never move.
        let previous = self.state.get_untracked();
        self.state
            .set(from_snapshot_preserving_slots(snapshot, &previous));
        self.loading.set(false);
        Ok(())
    }

    fn fail(&self, error: String) {
        self.connection.set(ConnectionState::Offline);
        self.loading.set(false);
        self.state.update(|state| {
            state.integrity = IntegrityState::Disconnected;
            state.last_error = Some(error);
        });
    }
}

fn endpoint(base: &str, path: &str) -> String {
    if base.trim().is_empty() {
        path.to_owned()
    } else {
        format!("{}{}", base.trim_end_matches('/'), path)
    }
}

#[cfg(test)]
mod tests {
    use super::endpoint;

    #[test]
    fn endpoint_preserves_same_origin_and_direct_daemon_paths() {
        assert_eq!(
            endpoint("", "/v1/observatory/snapshot"),
            "/v1/observatory/snapshot"
        );
        assert_eq!(
            endpoint("http://127.0.0.1:4780/", "/v1/observatory/snapshot"),
            "http://127.0.0.1:4780/v1/observatory/snapshot"
        );
    }
}
