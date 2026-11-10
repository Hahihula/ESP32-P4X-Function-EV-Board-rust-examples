//! SNTP (simple network time protocol) packets and UTC calendar time.
//!
//! Send [`request`] in a UDP datagram to port [`PORT`] of a time server,
//! for example `pool.ntp.org`, and pass the reply to [`parse_response`] to
//! get the time as Unix seconds. [`DateTime`] turns that into a calendar
//! date and time.
//!
//! Hardware-independent: builds and is unit-tested on a PC.

use core::fmt;

/// The NTP server port.
pub const PORT: u16 = 123;
/// Length of an NTP packet without extensions.
pub const PACKET_LEN: usize = 48;

/// Seconds from the NTP epoch (1900-01-01) to the Unix epoch (1970-01-01).
const NTP_TO_UNIX: u64 = 2_208_988_800;
/// First byte of a request: leap indicator 0, version 4, mode 3 (client).
const CLIENT_HEADER: u8 = (4 << 3) | 3;
/// Mode of a server reply.
const MODE_SERVER: u8 = 4;
/// Offset of the transmit timestamp's seconds field.
const TRANSMIT_SECONDS: usize = 40;

/// A client request: all zero except the header.
pub fn request() -> [u8; PACKET_LEN] {
    let mut packet = [0; PACKET_LEN];
    packet[0] = CLIENT_HEADER;
    packet
}

/// The server's transmit time, in Unix seconds, from a reply. `None` if the
/// packet is too short, is not a server reply, or is a "kiss of death"
/// (stratum 0: the server asks the client to back off).
pub fn parse_response(packet: &[u8]) -> Option<u64> {
    if packet.len() < PACKET_LEN || packet[0] & 0x07 != MODE_SERVER || packet[1] == 0 {
        return None;
    }
    let seconds: [u8; 4] = packet[TRANSMIT_SECONDS..TRANSMIT_SECONDS + 4]
        .try_into()
        .ok()?;
    u64::from(u32::from_be_bytes(seconds)).checked_sub(NTP_TO_UNIX)
}

/// A UTC date and time. Displays as `2026-11-10 14:27:39 UTC`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(missing_docs, reason = "the field names say it all")]
pub struct DateTime {
    pub year: i32,
    /// 1..=12
    pub month: u8,
    /// 1..=31
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
}

impl DateTime {
    /// Converts Unix seconds (since 1970-01-01 00:00:00 UTC) to a date and
    /// time, with the proleptic Gregorian calendar.
    pub fn from_unix(seconds: u64) -> Self {
        let days = seconds / 86_400;
        let time = seconds % 86_400;
        let (year, month, day) = civil_from_days(days as i64);
        Self {
            year,
            month,
            day,
            hour: (time / 3600) as u8,
            minute: (time / 60 % 60) as u8,
            second: (time % 60) as u8,
        }
    }
}

impl fmt::Display for DateTime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02} UTC",
            self.year, self.month, self.day, self.hour, self.minute, self.second
        )
    }
}

/// Year, month and day of the day `days` after 1970-01-01. Howard
/// Hinnant's algorithm: count in 400-year eras that start on 1 March, so
/// the leap day falls at the end of the year.
fn civil_from_days(days: i64) -> (i32, u8, u8) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_from_march = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_from_march + 2) / 5 + 1;
    let month = if month_from_march < 10 {
        month_from_march + 3
    } else {
        month_from_march - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year as i32, month as u8, day as u8)
}

#[cfg(test)]
mod tests {
    extern crate std;
    use std::string::ToString;

    use super::*;

    fn reply(mode: u8, stratum: u8, ntp_seconds: u32) -> [u8; PACKET_LEN] {
        let mut packet = [0; PACKET_LEN];
        packet[0] = (4 << 3) | mode;
        packet[1] = stratum;
        packet[TRANSMIT_SECONDS..TRANSMIT_SECONDS + 4].copy_from_slice(&ntp_seconds.to_be_bytes());
        packet
    }

    #[test]
    fn request_header() {
        let packet = request();
        assert_eq!(packet[0], 0x23);
        assert!(packet[1..].iter().all(|&b| b == 0));
    }

    #[test]
    fn parses_server_reply() {
        let unix = 1_794_320_859; // 2026-11-10 14:27:39 UTC
        let packet = reply(4, 2, (unix + NTP_TO_UNIX) as u32);
        assert_eq!(parse_response(&packet), Some(unix));
    }

    #[test]
    fn rejects_bad_replies() {
        assert_eq!(parse_response(&reply(3, 2, 3_900_000_000)), None); // client mode
        assert_eq!(parse_response(&reply(4, 0, 3_900_000_000)), None); // kiss of death
        assert_eq!(parse_response(&[0x24; 20]), None); // too short
    }

    #[test]
    fn calendar_dates() {
        assert_eq!(
            DateTime::from_unix(0).to_string(),
            "1970-01-01 00:00:00 UTC"
        );
        assert_eq!(
            DateTime::from_unix(1_794_320_859).to_string(),
            "2026-11-10 14:27:39 UTC"
        );
        // Leap day, and the day after.
        assert_eq!(
            DateTime::from_unix(951_782_400).to_string(),
            "2000-02-29 00:00:00 UTC"
        );
        assert_eq!(
            DateTime::from_unix(951_868_799).to_string(),
            "2000-02-29 23:59:59 UTC"
        );
        assert_eq!(
            DateTime::from_unix(951_868_800).to_string(),
            "2000-03-01 00:00:00 UTC"
        );
    }
}
