use smc_lib::io::IOService;

const CPU_TEMPERATURE_KEYS: &[[u8; 4]] = &[*b"TC0P", *b"TC0F"];
const GPU_TEMPERATURE_KEYS: &[[u8; 4]] = &[*b"TG0P", *b"TGDD"];
const GPU_POWER_KEYS: &[[u8; 4]] = &[*b"PG0R"];

pub(crate) fn cpu_package_temperature() -> Option<f32> {
    let service = IOService::init().ok()?;
    read_sensor(&service, CPU_TEMPERATURE_KEYS, -50.0, 200.0)
}

pub(crate) fn gpu_metrics() -> (Option<f32>, Option<f32>) {
    let Ok(service) = IOService::init() else {
        return (None, None);
    };

    (
        read_sensor(&service, GPU_TEMPERATURE_KEYS, -50.0, 200.0),
        read_sensor(&service, GPU_POWER_KEYS, 0.0, 2_000.0),
    )
}

fn read_sensor(service: &IOService, keys: &[[u8; 4]], minimum: f32, maximum: f32) -> Option<f32> {
    keys.iter().find_map(|key| {
        let value = service.read_key(key).ok()?;
        decode_value(&value.data_type, value.valid_bytes(), minimum, maximum)
    })
}

fn decode_value(data_type: &[u8; 4], bytes: &[u8], minimum: f32, maximum: f32) -> Option<f32> {
    if *data_type == *b"flt " {
        let raw: [u8; 4] = bytes.get(..4)?.try_into().ok()?;
        return [f32::from_le_bytes(raw), f32::from_be_bytes(raw)]
            .into_iter()
            .filter(|value| value.is_finite() && (minimum..=maximum).contains(value))
            .max_by(|left, right| left.abs().total_cmp(&right.abs()));
    }

    let signed = match *data_type {
        [b's', b'p', _, _] => true,
        [b'f', b'p', _, _] => false,
        _ => return None,
    };
    let fraction_bits = hex_digit(data_type[3])?;
    let expected_bits = if signed { 15 } else { 16 };
    if u16::from(hex_digit(data_type[2])?) + u16::from(fraction_bits) != expected_bits {
        return None;
    }

    let scale = (1_u32 << fraction_bits) as f32;
    if signed {
        let raw = i16::from_be_bytes(bytes.get(..2)?.try_into().ok()?);
        Some(f32::from(raw) / scale)
    } else {
        let raw = u16::from_be_bytes(bytes.get(..2)?.try_into().ok()?);
        Some(f32::from(raw) / scale)
    }
    .filter(|value| value.is_finite() && (minimum..=maximum).contains(value))
}

fn hex_digit(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::decode_value;

    #[test]
    fn decodes_signed_fixed_point_temperature() {
        let value = decode_value(b"sp78", &[0x37, 0x80], -50.0, 200.0);
        assert_eq!(value, Some(55.5));
    }

    #[test]
    fn accepts_both_float_byte_orders() {
        assert_eq!(
            decode_value(b"flt ", &[0x00, 0x00, 0x48, 0x42], 0.0, 100.0),
            Some(50.0)
        );
        assert_eq!(
            decode_value(b"flt ", &[0x42, 0x48, 0x00, 0x00], 0.0, 100.0),
            Some(50.0)
        );
    }

    #[test]
    fn rejects_out_of_range_sensor_values() {
        assert!(decode_value(b"sp78", &[0xCD, 0x00], -50.0, 200.0).is_none());
    }
}
