use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Serialize)]
pub(crate) struct Output {
    pub(crate) artifacts: Artifacts,
    pub(crate) sections: Vec<Section>,
}

#[derive(Serialize)]
pub(crate) struct Artifacts {
    pub(crate) canonical_transaction_sha256: String,
    pub(crate) canonical_transaction_bytes: usize,
}

#[derive(Serialize)]
pub(crate) struct Section {
    pub(crate) id: &'static str,
    pub(crate) name: &'static str,
    pub(crate) sample_counts: BTreeMap<&'static str, usize>,
    pub(crate) metrics: Vec<Metric>,
    pub(crate) assertions: Vec<Assertion>,
    pub(crate) notes: Vec<String>,
}

#[derive(Serialize)]
pub(crate) struct Metric {
    pub(crate) id: &'static str,
    pub(crate) name: String,
    pub(crate) value: f64,
    pub(crate) unit: &'static str,
    pub(crate) statistic: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) target: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) target_direction: Option<&'static str>,
}

#[derive(Serialize)]
pub(crate) struct Assertion {
    pub(crate) id: &'static str,
    pub(crate) name: &'static str,
    pub(crate) passed: bool,
    pub(crate) detail: String,
}

pub(crate) fn metric(
    name: impl Into<String>,
    value: f64,
    unit: &'static str,
    statistic: &'static str,
) -> Metric {
    let name = name.into();
    let id = match name.as_str() {
        "abstract state generation" => "abstract_state_generation_ms",
        "protobuf serialization" => "protobuf_serialization_ms",
        "serialized transaction size" => "serialized_transaction_bytes",
        "disconnect immediately before event receipt" => "disconnect_before_event_receipt_ms",
        "event receipt through settled side effect" => "event_to_settled_side_effect_ms",
        "in-process cached DUPLICATE response" => "cached_duplicate_response_ms",
        "lost ACK wire reconnect through DUPLICATE acknowledgement" => "lost_ack_wire_duplicate_ms",
        "mid-resource reconnect and exact replay" => "mid_resource_reconnect_ms",
        "mid-transaction frame discard and atomic replay" => "mid_transaction_codec_replay_ms",
        "mid-transaction wire disconnect and exact atomic replay" => {
            "mid_transaction_wire_replay_ms"
        }
        "partial EVENT disconnect and one processed replay" => "partial_event_wire_replay_ms",
        "resume beyond journal retention" => "resume_beyond_retention_ms",
        "resume within journal retention" => "resume_within_retention_ms",
        "embedded SRUI PTY exact ANSI capture and framing" => "embedded_pty_interaction_ms",
        "standalone PTY exact ANSI interaction" => "standalone_pty_interaction_ms",
        "terminal reconnect retention-loss decision" => "terminal_retention_loss_ms",
        "terminal payload" => "terminal_payload_bytes",
        "embedded terminal frame count" => "embedded_terminal_frame_count",
        _ => panic!("metric `{name}` is missing a stable benchmark ID"),
    };
    Metric {
        id,
        name,
        value,
        unit,
        statistic,
        target: None,
        target_direction: None,
    }
}

pub(crate) fn percentile(mut values: Vec<f64>, fraction: f64) -> f64 {
    values.sort_by(f64::total_cmp);
    let index = ((values.len() - 1) as f64 * fraction).round() as usize;
    values[index.min(values.len() - 1)]
}

pub(crate) fn p50(values: Vec<f64>) -> f64 {
    percentile(values, 0.50)
}

pub(crate) fn push_timing_distributions(
    metrics: &mut Vec<Metric>,
    timings: BTreeMap<&'static str, Vec<f64>>,
) {
    for (name, values) in timings {
        metrics.push(metric(name, p50(values.clone()), "ms", "p50"));
        metrics.push(metric(name, percentile(values.clone(), 0.95), "ms", "p95"));
        metrics.push(metric(name, percentile(values, 0.99), "ms", "p99"));
    }
}
