// SPDX-FileCopyrightText: Copyright Logically Magic Inc. <hello@logicallymagic.com>
// SPDX-FileCopyrightText: Copyright Hamid Jahanzad <Hamid@Jahanzad.com>
// SPDX-FileCopyrightText: Copyright Contributors to the Bill8:Billing
// SPDX-License-Identifier: Apache-2.0

//! Sentin8:Bus consumers — the event-driven wiring (billing DESIGN.md §7).
//!
//! Each loop follows the platform SSE pattern: connect to the family
//! stream with a stable consumer group, dispatch matched events, ACK,
//! reconnect with a delay on drop. The dispatch closure is async and
//! captures its own `AppState` clone; per-event tenant context is set on
//! the same connection as the handler's writes.

use std::time::Duration;

use futures_util::StreamExt;

use crate::handlers::AppState;

/// Reconnect delay when the Bus SSE connection drops.
const RECONNECT_DELAY: Duration = Duration::from_secs(2);

/// The parsed envelope of a Bus `data:` line.
pub struct Envelope {
    pub env_id: uuid::Uuid,
    pub event_type: String,
    pub payload: serde_json::Value,
}

/// Extract the envelope fields from a message payload.
fn parse_envelope(msg: &serde_json::Value) -> Option<Envelope> {
    let env_id = msg
        .get("env_id")
        .and_then(|v| v.as_str())
        .and_then(|s| uuid::Uuid::parse_str(s).ok())?;
    // The event type may sit at the top level or under `payload`.
    let event_type = msg
        .get("event_type")
        .and_then(|v| v.as_str())
        .or_else(|| {
            msg.get("payload")
                .and_then(|p| p.get("event_type"))
                .and_then(|v| v.as_str())
        })?
        .to_string();
    Some(Envelope {
        env_id,
        event_type,
        payload: msg.get("payload").cloned().unwrap_or_else(|| msg.clone()),
    })
}

/// Spawn a consumer loop for one family + group. The `dispatch` closure
/// captures everything it needs (the state clone, the reaction call)
/// and runs inline on the consumer task — each event's work is bounded
/// (retry budgets live in Bus, not the consumer).
pub fn spawn_family_consumer<F, Fut>(
    bus_client: sentin8::bus::client::BusClient,
    family: &'static str,
    consumer_group: &'static str,
    dispatch: F,
) where
    F: Fn(Envelope) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = ()> + Send,
{
    tokio::spawn(async move {
        tracing::info!(family, consumer_group, "bus consumer started");
        loop {
            match bus_client.sse_consume(family, consumer_group).await {
                Ok(resp) => {
                    tracing::info!(family, "bus SSE connected");
                    let mut stream = resp.bytes_stream();
                    let mut buffer = String::new();
                    while let Some(chunk) = stream.next().await {
                        let chunk = match chunk {
                            Ok(c) => c,
                            Err(e) => {
                                tracing::warn!(error = %e, family, "bus SSE stream error");
                                break;
                            }
                        };
                        buffer.push_str(&String::from_utf8_lossy(&chunk));
                        while let Some(idx) = buffer.find('\n') {
                            let line = buffer[..idx].to_string();
                            buffer.drain(..=idx);
                            let Some(data) = line.strip_prefix("data: ") else {
                                continue;
                            };
                            if let Ok(msg) = serde_json::from_str::<serde_json::Value>(data.trim()) {
                                if let Some(envelope) = parse_envelope(&msg) {
                                    dispatch(envelope).await;
                                }
                            }
                        }
                    }
                    tracing::info!(family, "bus SSE disconnected — reconnecting");
                }
                Err(e) => {
                    tracing::warn!(error = %e, family, "bus SSE consume failed — reconnecting");
                }
            }
            tokio::time::sleep(RECONNECT_DELAY).await;
        }
    });
}

/// Spawn all the billing service's consumer loops (called once from
/// `build_state`):
///
/// - `payments` family, group `bill8:billing:invoice` — marks
///   invoices paid when their charges succeed (BIL-6; recovery restores
///   the subscription to `active`).
pub fn spawn_consumers(state: AppState) {
    let bus = state.bus_client.clone();

    // Invoice-paid consumer: reacts to charge:succeeded for the
    // billing-issued charges (idempotency_key `invoice-{id}`).
    let invoice_state = state;
    spawn_family_consumer(bus, "payments", "bill8:billing:invoice", move |envelope: Envelope| {
        let state = invoice_state.clone();
        async move {
            if envelope.event_type == "bill8:payments:charge:succeeded" {
                crate::handlers::billing_richness::handle_charge_succeeded(
                    &state,
                    envelope.env_id,
                    envelope.payload,
                )
                .await;
            }
        }
    });
}