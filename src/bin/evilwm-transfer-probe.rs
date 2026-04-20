use std::time::Duration;

use serde_json::{Value, json};

fn main() {
    if let Err(error) = run() {
        eprintln!("evilwm-transfer-probe: {error}");
        std::process::exit(1);
    }
}

struct ProbeArgs {
    mode: String,
    mime: String,
    payload: String,
    timeout: Duration,
}

struct SelectionSourceSummary {
    offered_mimes: Vec<String>,
    serial_used: Option<u32>,
    selection_set: bool,
    send_count: u32,
    bytes_written: usize,
    error: Option<String>,
}

struct SelectionSinkSummary {
    received_mimes: Vec<String>,
    offer_received: bool,
    receive_requested: bool,
    payload_read_finished: bool,
    chosen_mime: Option<String>,
    payload: Option<Vec<u8>>,
    error: Option<String>,
}

struct DndSourceSummary {
    offered_mimes: Vec<String>,
    pointer_serial_obtained: bool,
    start_drag_attempted: bool,
    send_count: u32,
    bytes_written: usize,
    blocked_reason: Option<String>,
    error: Option<String>,
}

struct DndTargetSummary {
    enter_received: bool,
    offered_mimes: Vec<String>,
    offer_received: bool,
    receive_requested: bool,
    payload_read_finished: bool,
    chosen_mime: Option<String>,
    drop_received: bool,
    payload: Option<Vec<u8>>,
    error: Option<String>,
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let Some(args) = parse_args()? else {
        return Ok(());
    };

    match args.mode.as_str() {
        "clipboard-source" => {
            let result = evilwm::probe::transfer::clipboard::run_clipboard_source(
                args.payload.as_bytes(),
                &args.mime,
                args.timeout,
            );
            emit_json(selection_source_json(
                &args.mode,
                &args.mime,
                SelectionSourceSummary {
                    offered_mimes: result.offered_mimes,
                    serial_used: result.serial_used,
                    selection_set: result.selection_set,
                    send_count: result.send_count,
                    bytes_written: result.bytes_written,
                    error: result.error,
                },
            ))?;
        }
        "clipboard-sink" => {
            let result =
                evilwm::probe::transfer::clipboard::run_clipboard_sink(&args.mime, args.timeout);
            emit_json(selection_sink_json(
                &args.mode,
                &args.mime,
                SelectionSinkSummary {
                    received_mimes: result.received_mimes,
                    offer_received: result.offer_received,
                    receive_requested: result.receive_requested,
                    payload_read_finished: result.payload_read_finished,
                    chosen_mime: result.chosen_mime,
                    payload: result.payload,
                    error: result.error,
                },
            ))?;
        }
        "primary-source" => {
            let result = evilwm::probe::transfer::primary::run_primary_source(
                args.payload.as_bytes(),
                &args.mime,
                args.timeout,
            );
            emit_json(selection_source_json(
                &args.mode,
                &args.mime,
                SelectionSourceSummary {
                    offered_mimes: result.offered_mimes,
                    serial_used: result.serial_used,
                    selection_set: result.selection_set,
                    send_count: result.send_count,
                    bytes_written: result.bytes_written,
                    error: result.error,
                },
            ))?;
        }
        "primary-sink" => {
            let result =
                evilwm::probe::transfer::primary::run_primary_sink(&args.mime, args.timeout);
            emit_json(selection_sink_json(
                &args.mode,
                &args.mime,
                SelectionSinkSummary {
                    received_mimes: result.received_mimes,
                    offer_received: result.offer_received,
                    receive_requested: result.receive_requested,
                    payload_read_finished: result.payload_read_finished,
                    chosen_mime: result.chosen_mime,
                    payload: result.payload,
                    error: result.error,
                },
            ))?;
        }
        "dnd-source" => {
            let result = evilwm::probe::transfer::dnd::run_dnd_source(
                args.payload.as_bytes(),
                &args.mime,
                args.timeout,
            );
            emit_json(dnd_source_json(
                &args.mode,
                &args.mime,
                DndSourceSummary {
                    offered_mimes: result.offered_mimes,
                    pointer_serial_obtained: result.pointer_serial_obtained,
                    start_drag_attempted: result.start_drag_attempted,
                    send_count: result.send_count,
                    bytes_written: result.bytes_written,
                    blocked_reason: result.blocked_reason,
                    error: result.error,
                },
            ))?;
        }
        "dnd-target" => {
            let result = evilwm::probe::transfer::dnd::run_dnd_target(&args.mime, args.timeout);
            emit_json(dnd_target_json(
                &args.mode,
                &args.mime,
                DndTargetSummary {
                    enter_received: result.enter_received,
                    offered_mimes: result.offered_mimes,
                    offer_received: result.offer_received,
                    receive_requested: result.receive_requested,
                    payload_read_finished: result.payload_read_finished,
                    chosen_mime: result.chosen_mime,
                    drop_received: result.drop_received,
                    payload: result.payload,
                    error: result.error,
                },
            ))?;
        }
        _ => return Err(format!("unknown mode: {}", args.mode).into()),
    }

    Ok(())
}

fn parse_args() -> Result<Option<ProbeArgs>, Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let Some(mode) = args.next() else {
        print_usage();
        return Ok(None);
    };
    if mode == "-h" || mode == "--help" {
        print_usage();
        return Ok(None);
    }

    let mut mime = String::from("text/plain;charset=utf-8");
    let mut payload = String::from("evilwm transfer probe");
    let mut timeout_ms = 3_000_u64;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--mime" => mime = args.next().ok_or("--mime requires a value")?,
            "--payload" => payload = args.next().ok_or("--payload requires a value")?,
            "--timeout-ms" => {
                timeout_ms = args
                    .next()
                    .ok_or("--timeout-ms requires a value")?
                    .parse()?;
            }
            "-h" | "--help" => {
                print_usage();
                return Ok(None);
            }
            other => return Err(format!("unknown argument: {other}").into()),
        }
    }

    Ok(Some(ProbeArgs {
        mode,
        mime,
        payload,
        timeout: Duration::from_millis(timeout_ms),
    }))
}

fn emit_json(value: Value) -> Result<(), Box<dyn std::error::Error>> {
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}

fn payload_string(payload: Option<&Vec<u8>>) -> Option<String> {
    payload.map(|bytes| String::from_utf8_lossy(bytes).to_string())
}

fn payload_len(payload: Option<&Vec<u8>>) -> Option<usize> {
    payload.map(Vec::len)
}

fn selection_source_stage(
    error: &Option<String>,
    serial_used: Option<u32>,
    selection_set: bool,
    send_count: u32,
) -> &'static str {
    if error.is_some() {
        "error"
    } else if send_count > 0 {
        "payload_sent"
    } else if selection_set {
        "selection_set"
    } else if serial_used.is_some() {
        "focus_observed"
    } else {
        "waiting_for_focus"
    }
}

fn selection_sink_stage(
    error: &Option<String>,
    offer_received: bool,
    receive_requested: bool,
    payload_read_finished: bool,
) -> &'static str {
    if error.is_some() {
        "error"
    } else if payload_read_finished {
        "payload_read_finished"
    } else if receive_requested {
        "receive_requested"
    } else if offer_received {
        "offer_received"
    } else {
        "waiting_for_offer"
    }
}

fn dnd_source_stage(
    error: &Option<String>,
    pointer_serial_obtained: bool,
    start_drag_attempted: bool,
    send_count: u32,
) -> &'static str {
    if error.is_some() {
        "error"
    } else if send_count > 0 {
        "payload_sent"
    } else if start_drag_attempted {
        "drag_started"
    } else if pointer_serial_obtained {
        "pointer_serial_obtained"
    } else {
        "waiting_for_pointer_button"
    }
}

fn dnd_target_stage(
    error: &Option<String>,
    enter_received: bool,
    offer_received: bool,
    receive_requested: bool,
    payload_read_finished: bool,
) -> &'static str {
    if error.is_some() {
        "error"
    } else if payload_read_finished {
        "payload_read_finished"
    } else if receive_requested {
        "receive_requested"
    } else if offer_received {
        "offer_received"
    } else if enter_received {
        "enter_received"
    } else {
        "waiting_for_enter"
    }
}

fn selection_source_json(mode: &str, mime: &str, summary: SelectionSourceSummary) -> Value {
    json!({
        "mode": mode,
        "mime": mime,
        "offered_mimes": summary.offered_mimes,
        "focus_observed": summary.serial_used.is_some(),
        "selection_set": summary.selection_set,
        "serial_used": summary.serial_used,
        "send_count": summary.send_count,
        "bytes_written": summary.bytes_written,
        "success": summary.error.is_none() && summary.send_count > 0,
        "stage": selection_source_stage(
            &summary.error,
            summary.serial_used,
            summary.selection_set,
            summary.send_count,
        ),
        "error": summary.error,
    })
}

fn selection_sink_json(mode: &str, mime: &str, summary: SelectionSinkSummary) -> Value {
    json!({
        "mode": mode,
        "mime": mime,
        "received_mimes": summary.received_mimes,
        "offer_received": summary.offer_received,
        "receive_requested": summary.receive_requested,
        "payload_read_finished": summary.payload_read_finished,
        "chosen_mime": summary.chosen_mime,
        "payload": payload_string(summary.payload.as_ref()),
        "payload_len": payload_len(summary.payload.as_ref()),
        "success": summary.error.is_none() && summary.payload.is_some(),
        "stage": selection_sink_stage(
            &summary.error,
            summary.offer_received,
            summary.receive_requested,
            summary.payload_read_finished,
        ),
        "error": summary.error,
    })
}

fn dnd_source_json(mode: &str, mime: &str, summary: DndSourceSummary) -> Value {
    json!({
        "mode": mode,
        "mime": mime,
        "offered_mimes": summary.offered_mimes,
        "pointer_serial_obtained": summary.pointer_serial_obtained,
        "start_drag_attempted": summary.start_drag_attempted,
        "send_count": summary.send_count,
        "bytes_written": summary.bytes_written,
        "blocked_reason": summary.blocked_reason,
        "success": summary.start_drag_attempted && summary.send_count > 0 && summary.error.is_none(),
        "stage": dnd_source_stage(
            &summary.error,
            summary.pointer_serial_obtained,
            summary.start_drag_attempted,
            summary.send_count,
        ),
        "error": summary.error,
    })
}

fn dnd_target_json(mode: &str, mime: &str, summary: DndTargetSummary) -> Value {
    json!({
        "mode": mode,
        "mime": mime,
        "enter_received": summary.enter_received,
        "offered_mimes": summary.offered_mimes,
        "offer_received": summary.offer_received,
        "receive_requested": summary.receive_requested,
        "payload_read_finished": summary.payload_read_finished,
        "chosen_mime": summary.chosen_mime,
        "drop_received": summary.drop_received,
        "payload": payload_string(summary.payload.as_ref()),
        "payload_len": payload_len(summary.payload.as_ref()),
        "success": summary.enter_received
            && summary.drop_received
            && summary.payload.is_some()
            && summary.error.is_none(),
        "stage": dnd_target_stage(
            &summary.error,
            summary.enter_received,
            summary.offer_received,
            summary.receive_requested,
            summary.payload_read_finished,
        ),
        "error": summary.error,
    })
}

fn print_usage() {
    println!(
        "Usage: evilwm-transfer-probe <clipboard-source|clipboard-sink|primary-source|primary-sink|dnd-source|dnd-target> [--mime TYPE] [--payload TEXT] [--timeout-ms N]
  clipboard-source  acquire clipboard ownership and serve payload
  clipboard-sink    read current clipboard selection
  primary-source    acquire primary selection and serve payload
  primary-sink      read current primary selection
  dnd-source        offer a DnD payload; requires a pointer button press to start_drag
  dnd-target        map a surface and wait for a DnD drop (requires a running source)"
    );
}
