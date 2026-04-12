#![allow(dead_code)]

#[path = "../aethos_core/mod.rs"]
mod aethos_core;

use std::process::ExitCode;

use aethos_core::ble_discovery::{
    accept_v2_wakeup_observation, canonical_ble_primary_service_uuid,
};

const AETHOS_PRIMARY_UUID_LE: [u8; 16] = [
    0x4e, 0xee, 0x0d, 0xd2, 0x6c, 0x0e, 0xf7, 0x87, 0xf9, 0x50, 0x29, 0x5a, 0x85, 0xa5, 0x1a, 0x18,
];

fn main() -> ExitCode {
    let args = match parse_args(std::env::args().skip(1).collect()) {
        Ok(args) => args,
        Err(message) => {
            eprintln!("{message}");
            eprintln!("{}", usage());
            return ExitCode::FAILURE;
        }
    };

    let primary = match decode_hex(&args.primary_hex) {
        Ok(bytes) => bytes,
        Err(err) => {
            eprintln!("invalid --primary hex: {err}");
            return ExitCode::FAILURE;
        }
    };

    println!("BLE Wakeup Inspector v2");
    println!("- canonical_uuid={}", canonical_ble_primary_service_uuid());
    println!("- primary_len={}", primary.len());

    println!("\nPrimary advertisement AD structures:");
    print_ad_summary(&primary);

    let now_ms = 0;
    let ble_address = args.ble_address.as_deref().unwrap_or("00:00:00:00:00:00");
    match accept_v2_wakeup_observation(&primary, ble_address, now_ms, None, "inspector") {
        Ok(signal) => {
            println!("\nVerdict: ACCEPTED (v2 wakeup hint)");
            println!("- peer_hint={}", signal.peer_hint);
            println!("- source={}", signal.source);
            ExitCode::SUCCESS
        }
        Err(rejection) => {
            println!("\nVerdict: REJECTED");
            println!("- reason_code={}", rejection.reason_code);
            println!("- reason_label={}", rejection.reason_label);
            println!("- detail={}", rejection.detail);
            println!("- hint={}", remediation_hint(rejection.reason_code));
            ExitCode::from(2)
        }
    }
}

struct ParsedArgs {
    primary_hex: String,
    ble_address: Option<String>,
}

fn parse_args(args: Vec<String>) -> Result<ParsedArgs, String> {
    if args.is_empty() || args.iter().any(|arg| arg == "-h" || arg == "--help") {
        return Err("missing required args".to_string());
    }

    let mut primary_hex: Option<String> = None;
    let mut ble_address: Option<String> = None;
    let mut index = 0usize;
    while index < args.len() {
        match args[index].as_str() {
            "--primary" => {
                let Some(value) = args.get(index + 1) else {
                    return Err("--primary requires a value".to_string());
                };
                primary_hex = Some(value.to_string());
                index += 2;
            }
            "--address" | "--addr" => {
                let Some(value) = args.get(index + 1) else {
                    return Err("--address requires a value".to_string());
                };
                ble_address = Some(value.to_string());
                index += 2;
            }
            unexpected => {
                return Err(format!("unknown arg: {unexpected}"));
            }
        }
    }

    let Some(primary_hex) = primary_hex else {
        return Err("--primary is required".to_string());
    };

    Ok(ParsedArgs {
        primary_hex,
        ble_address,
    })
}

fn usage() -> &'static str {
    "usage: cargo run --bin ble-identity-inspector -- --primary <hex> [--address <ble-addr>]\n\nIn v2 BLE is a discovery-only wakeup hint. Only the primary advertisement\nis inspected for the Aethos UUID in AD type 0x06/0x07. Scan response and\nidentity payloads are not used.\n\nexample:\n  cargo run --bin ble-identity-inspector -- --primary 11074eee0dd26c0ef787f950295a85a51a18\n  cargo run --bin ble-identity-inspector -- --primary 11074eee0dd26c0ef787f950295a85a51a18 --address AA:BB:CC:DD:EE:FF"
}

fn decode_hex(raw: &str) -> Result<Vec<u8>, String> {
    let mut cleaned = String::with_capacity(raw.len());
    for ch in raw.chars() {
        if ch.is_ascii_hexdigit() {
            cleaned.push(ch);
            continue;
        }
        if ch == ':' || ch == '-' || ch == ' ' || ch == '\n' || ch == '\t' {
            continue;
        }
        return Err(format!("unsupported character '{ch}'"));
    }
    if cleaned.is_empty() {
        return Ok(Vec::new());
    }
    if !cleaned.len().is_multiple_of(2) {
        return Err("hex string must contain an even number of digits".to_string());
    }
    let mut out = Vec::with_capacity(cleaned.len() / 2);
    for idx in (0..cleaned.len()).step_by(2) {
        let byte = u8::from_str_radix(&cleaned[idx..idx + 2], 16)
            .map_err(|err| format!("byte {} parse error: {err}", idx / 2))?;
        out.push(byte);
    }
    Ok(out)
}

fn print_ad_summary(raw: &[u8]) {
    let entries = match parse_ad_structures(raw) {
        Ok(entries) => entries,
        Err(err) => {
            println!("- malformed: {err}");
            return;
        }
    };
    if entries.is_empty() {
        println!("- (none)");
        return;
    }

    for (idx, (ad_type, data)) in entries.iter().enumerate() {
        println!(
            "- #{idx} type=0x{ad_type:02x} len={} data={}",
            data.len(),
            bytes_to_hex_lower(data)
        );
        if *ad_type == 0x06 || *ad_type == 0x07 {
            if data.len() % 16 != 0 {
                println!("  uuid_list_error=length_not_multiple_of_16");
                continue;
            }
            for (uuid_idx, chunk) in data.chunks_exact(16).enumerate() {
                let matched = chunk == AETHOS_PRIMARY_UUID_LE;
                println!(
                    "  uuid[{uuid_idx}]={}{}",
                    bytes_to_hex_lower(chunk),
                    if matched { " (aethos-primary)" } else { "" }
                );
            }
        }
        if *ad_type == 0x21 {
            // v2: service data is ignored by the scanner, but still useful to
            // display for diagnostic purposes during the migration window.
            if data.len() < 16 {
                println!("  service_data_note=short_entry (ignored in v2)");
                continue;
            }
            let uuid = &data[0..16];
            let payload = &data[16..];
            let uuid_match = uuid == AETHOS_PRIMARY_UUID_LE;
            println!(
                "  service_data_uuid={}{} payload_len={} (ignored in v2)",
                bytes_to_hex_lower(uuid),
                if uuid_match { " (aethos-primary)" } else { "" },
                payload.len()
            );
        }
    }
}

fn parse_ad_structures(raw: &[u8]) -> Result<Vec<(u8, Vec<u8>)>, String> {
    let mut out = Vec::new();
    let mut cursor = 0usize;
    while cursor < raw.len() {
        let len = raw[cursor] as usize;
        cursor += 1;
        if len == 0 {
            break;
        }
        if cursor + len > raw.len() {
            return Err(format!(
                "len byte at offset {} overruns buffer (len={} remaining={})",
                cursor - 1,
                len,
                raw.len().saturating_sub(cursor)
            ));
        }
        let ad_type = raw[cursor];
        let payload = raw[cursor + 1..cursor + len].to_vec();
        out.push((ad_type, payload));
        cursor += len;
    }
    Ok(out)
}

fn remediation_hint(reason_code: &str) -> &'static str {
    match reason_code {
        "missing_primary_service_uuid" => {
            "Primary advertisement must include AD type 0x06/0x07 with Aethos UUID in little-endian BLE order. See ble-identity-v2.md §4."
        }
        "malformed_primary_service_uuid_list" => {
            "AD type 0x06/0x07 data length must be an exact multiple of 16 bytes."
        }
        "malformed_ad_structure" => {
            "One or more AD structures are malformed (length byte does not match available bytes)."
        }
        _ => "See docs/protocol/ble-identity-v2.md for v2 wakeup hint acceptance rules.",
    }
}

fn bytes_to_hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{decode_hex, parse_ad_structures};
    use crate::aethos_core::ble_discovery::build_primary_uuid_list_ad;

    #[test]
    fn decode_hex_accepts_common_separators() {
        let bytes = decode_hex("11:07-4e ee").expect("decode");
        assert_eq!(bytes, vec![0x11, 0x07, 0x4e, 0xee]);
    }

    #[test]
    fn parser_reads_primary_uuid_ad() {
        let ad = build_primary_uuid_list_ad();
        let parsed = parse_ad_structures(&ad).expect("parse");
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].0, 0x07);
        assert_eq!(parsed[0].1.len(), 16);
    }
}
