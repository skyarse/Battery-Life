//! Read-only query formats. Protocol references and limitations: docs/PROTOCOLS.md.
//! No settings, pairing, firmware, EQ or memory writes are implemented.
use anyhow::{Context, Result, bail};
use hidapi::HidDevice;
use std::time::{Duration, Instant};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reading {
    pub percent: Option<u8>,
    pub level: Option<String>,
    pub charging: Option<bool>,
    pub approximate: bool,
}

pub fn compx_packet(command: u8) -> [u8; 17] {
    let mut p = [0; 17];
    p[0] = 8;
    p[1] = command;
    set_compx_checksum(&mut p);
    p
}

fn set_compx_checksum(p: &mut [u8; 17]) {
    p[16] = 0x55u8.wrapping_sub(p[..16].iter().fold(0u8, |sum, byte| sum.wrapping_add(*byte)));
}

fn compx_valid(p: &[u8], command: u8) -> bool {
    p.len() == 17 && p[0] == 8 && p[1] == command && p[2] == 0
        && p[5] <= 10 && p.iter().fold(0u8, |s, b| s.wrapping_add(*b)) == 0x55
}

pub fn parse_compx(p: &[u8]) -> Option<Reading> {
    if !compx_valid(p, 4) || p[5] < 2 || p[6] > 100 { return None; }
    let charging = match p[7] { 0 => Some(false), 1 => Some(true), _ => None };
    Some(Reading { percent: Some(p[6]), level: None, charging, approximate: true })
}

fn is_mchose_reply(report: &[u8]) -> bool {
    if report.len() < 4 || report[..2] != [0x55, 0x65] {
        return false;
    }
    // An echoed request has the same prefix as a status reply.
    !(report[2] == 1 && report[3..].iter().all(|byte| *byte == 0))
}

fn parse_mchose(report: &[u8]) -> Result<Reading> {
    if !is_mchose_reply(report) {
        bail!("Ответ MCHOSE не содержит статуса батареи");
    }
    let (percent, status) = (report[2], report[3]);
    match status {
        0x02 if percent <= 100 => Ok(Reading {
            percent: Some(percent),
            level: None,
            charging: Some(false),
            approximate: true,
        }),
        // The reported level can freeze while the cable is plugged in.
        0x00 | 0x03 => Ok(Reading {
            percent: None,
            level: None,
            charging: Some(true),
            approximate: false,
        }),
        0x1a => bail!("Приёмник сообщает, что наушники выключены"),
        _ => bail!("Неизвестный ответ MCHOSE о батарее (уровень {percent}, статус {status:#04x})"),
    }
}

pub fn parse_hidpp(p: &[u8], feature: u8, software_id: u8) -> Option<Reading> {
    if p.len() < 7 || !matches!(p[0], 0x10 | 0x11) || p[1] != 1 || p[2] != feature || p[3] != software_id || p[4] > 100 { return None; }
    // BATTERY_STATUS 0x1000 is coarse on the AA-powered G304/G305.
    Some(Reading { percent: Some(p[4]), level: None, charging: None, approximate: true })
}

pub fn parse_xbox_battery(p: &[u8]) -> Option<Reading> {
    if p.len() != 2 || p[0] != 4 || p[1] & !0x0f != 0 { return None; }
    let power_type = (p[1] >> 2) & 3;
    if power_type == 3 { return None; } // Unknown battery type.
    let level = match p[1] & 3 { 0 => "Критический", 1 => "Низкий", 2 => "Средний", _ => "Высокий" };
    // Four levels are not four exact percentages. USB power does not prove charging.
    Some(Reading { percent: None, level: Some(level.into()), charging: None, approximate: true })
}

fn response(dev: &HidDevice, budget: Duration, accept: impl Fn(&[u8]) -> bool) -> Result<Vec<u8>> {
    let until = Instant::now() + budget;
    let mut data = [0u8; 128];
    while Instant::now() < until {
        let remaining = until.saturating_duration_since(Instant::now()).as_millis().min(150) as i32;
        let n = dev.read_timeout(&mut data, remaining.max(1))?;
        if n > 0 && accept(&data[..n]) { return Ok(data[..n].to_vec()); }
    }
    bail!("Нет ответа: устройство выключено, спит или протокол не поддерживается")
}

fn write(dev: &HidDevice, data: &[u8]) -> Result<()> {
    let n = dev.write(data)?;
    if n != data.len() { bail!("Неполная отправка запроса: {n}/{}", data.len()); }
    Ok(())
}

pub fn zaopin(dev: &HidDevice, product_id: u16) -> Result<Reading> {
    // Compx receivers can use the same USB ID for different brands and models.
    let mut identify = compx_packet(1);
    identify[5] = 8;
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();
    identify[6..10].copy_from_slice(&nonce.to_le_bytes());
    set_compx_checksum(&mut identify);
    write(dev, &identify).context("Compx, запрос идентификации 0x01")?;
    let identity = response(dev, Duration::from_millis(600), |p| compx_valid(p, 1) && p[5] >= 7)
        .context("Compx, ответ идентификации 0x01")?;
    let (cid, mid) = (identity[10], identity[11]);
    check_zaopin_identity(product_id, cid, mid)?;
    std::thread::sleep(Duration::from_millis(10));
    // Confirm the mouse is online so a receiver's cached value is not treated as fresh.
    write(dev, &compx_packet(3)).context("Compx, запрос связи 0x03")?;
    let online = response(dev, Duration::from_millis(600), |p| compx_valid(p, 3) && p[5] >= 1)
        .with_context(|| format!("Compx, ответ о связи 0x03 (CID {cid}, MID {mid})"))?;
    if online[6] == 0 { bail!("Мышь спит или отключена от приёмника (CID {cid}, MID {mid})"); }
    std::thread::sleep(Duration::from_millis(10));
    write(dev, &compx_packet(4)).context("Compx, запрос заряда 0x04")?;
    let p = response(dev, Duration::from_millis(600), |p| parse_compx(p).is_some())
        .with_context(|| format!("Compx, ответ о заряде 0x04 (CID {cid}, MID {mid})"))?;
    Ok(parse_compx(&p).unwrap())
}

fn check_zaopin_identity(product_id: u16, cid: u8, mid: u8) -> Result<()> {
    // The reported Z2 Max uses the 3554:f517 receiver on usage ff02:0002,
    // but its valid handshake returns different model bytes. Limit the
    // exception to that receiver. Link and battery replies are still checked.
    if product_id != 0xf517 && (cid, mid) != (7, 5) {
        bail!("Приёмник не подтвердил модель ZAOPIN Z2 Max (CID {cid}, MID {mid})");
    }
    Ok(())
}

pub fn mchose(dev: &HidDevice) -> Result<Reading> {
    let mut request = [0u8; 64];
    request[..3].copy_from_slice(&[0x55, 0x65, 1]);
    write(dev, &request)?;
    let reply = response(dev, Duration::from_millis(1000), is_mchose_reply)?;
    parse_mchose(&reply)
}

pub fn logitech(dev: &HidDevice) -> Result<Reading> {
    const SOFTWARE: u8 = 0x0b;
    // Discover the index of feature 0x1000. Never assume it is always feature 5.
    write(dev, &[0x10, 1, 0, SOFTWARE, 0x10, 0, 0])?;
    let root = response(dev, Duration::from_millis(700), |p| p.len() >= 7 && matches!(p[0], 0x10 | 0x11) && p[1] == 1 && p[2] == 0 && p[3] == SOFTWARE)?;
    let feature = root[4];
    if feature == 0 { bail!("Мышь не сообщает BATTERY_STATUS 0x1000"); }
    write(dev, &[0x10, 1, feature, SOFTWARE, 0, 0, 0])?;
    let p = response(dev, Duration::from_millis(700), |p| parse_hidpp(p, feature, SOFTWARE).is_some())?;
    Ok(parse_hidpp(&p, feature, SOFTWARE).unwrap())
}

/// Passive only: no mode-switching, vibration or controller setup commands.
pub fn xbox(dev: &HidDevice) -> Result<Reading> {
    let p = response(dev, Duration::from_millis(700), |p| parse_xbox_battery(p).is_some())?;
    Ok(parse_xbox_battery(&p).unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reported_8k_receiver_does_not_require_the_reference_model_code() {
        assert!(check_zaopin_identity(0xf517, 7, 9).is_ok());
        assert!(check_zaopin_identity(0xf527, 7, 9).is_err());
        assert!(check_zaopin_identity(0xf527, 7, 5).is_ok());
    }
    fn compx_reply() -> [u8; 17] {
        let mut p = compx_packet(4);
        p[5] = 4; p[6] = 76; p[7] = 1;
        set_compx_checksum(&mut p);
        p
    }
    #[test]
    fn compx_rejects_corruption_echoes_and_other_commands() {
        let p = compx_reply();
        assert_eq!(parse_compx(&p).unwrap().percent, Some(76));
        assert_eq!(parse_compx(&p).unwrap().charging, Some(true));
        for i in 0..17 {
            let mut bad = p; bad[i] ^= 1;
            assert!(parse_compx(&bad).is_none(), "corrupted byte {i}");
        }
        assert!(parse_compx(&compx_packet(4)).is_none());
        assert!(parse_compx(&p[..8]).is_none());
    }
    #[test]
    fn mchose_does_not_show_request_echo_as_one_percent() {
        let mut request = [0; 64];
        request[..3].copy_from_slice(&[0x55, 0x65, 1]);
        assert!(!is_mchose_reply(&request));
        assert!(!is_mchose_reply(&[0x55, 0x11, 80, 0]));
    }

    #[test]
    fn mchose_uses_the_reported_power_state() {
        // Prefixes from captured V9 Pro dongle reports (see docs/PROTOCOLS.md).
        let battery = parse_mchose(&[0x55, 0x65, 0x5a, 0x02]).unwrap();
        assert_eq!(battery.percent, Some(90));
        assert!(battery.approximate);
        for frame in [[0x55, 0x65, 0x32, 0x03], [0x55, 0x65, 0x5a, 0x00]] {
            let charging = parse_mchose(&frame).unwrap();
            assert_eq!(charging.charging, Some(true));
            assert_eq!(charging.percent, None);
        }
        assert!(parse_mchose(&[0x55, 0x65, 0, 0x1a]).is_err());
        assert!(parse_mchose(&[0x55, 0x65, 101, 0x02]).is_err());
    }
    #[test]
    fn hidpp_matches_device_feature_and_request_id() {
        let p = [0x10, 1, 5, 0x0b, 90, 50, 0];
        assert_eq!(parse_hidpp(&p, 5, 0x0b).unwrap().percent, Some(90));
        assert!(parse_hidpp(&p, 6, 0x0b).is_none());
        assert!(parse_hidpp(&p, 5, 0x0c).is_none());
        assert!(parse_hidpp(&[0x10, 1, 0x8f, 0x0b, 90, 50, 0], 5, 0x0b).is_none());
    }
    #[test]
    fn xbox_reports_levels_without_inventing_percentages() {
        let r = parse_xbox_battery(&[4, 6]).unwrap();
        assert_eq!(r.percent, None);
        assert_eq!(r.level.as_deref(), Some("Средний"));
        assert!(parse_xbox_battery(&[4, 255]).is_none());
        assert!(parse_xbox_battery(&[4, 6, 0]).is_none());
        assert!(parse_xbox_battery(&[1, 6]).is_none());
    }
}
