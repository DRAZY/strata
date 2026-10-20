//! Local date and time, and the moment.js formats Obsidian writes them in
//! (daily note names like `YYYY-MM-DD`, `{{date:dddd, MMMM Do}}` in
//! templates), with moment's English names and week rules.

/// A local date and time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DateTime {
    pub year: i32,
    /// 1–12.
    pub month: u32,
    /// 1–31.
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
    pub millis: u32,
    /// Minutes east of UTC.
    pub offset: i32,
}

const MONTHS: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
const DAYS: [&str; 7] = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];

/// Days since 1970-01-01 of a proleptic Gregorian date.
fn days_from_civil(y: i32, m: u32, d: u32) -> i64 {
    let y = i64::from(y) - i64::from(m <= 2);
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let m = i64::from(m);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The date `days` after 1970-01-01.
fn civil_from_days(days: i64) -> (i32, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    ((yoe + era * 400 + i64::from(m <= 2)) as i32, m, d)
}

fn leap(y: i32) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

impl DateTime {
    /// The local time now.
    pub fn now() -> DateTime {
        local_now()
    }

    /// A date at midnight, UTC.
    #[cfg(test)]
    pub fn date(year: i32, month: u32, day: u32) -> DateTime {
        DateTime { year, month, day, hour: 0, minute: 0, second: 0, millis: 0, offset: 0 }
    }

    fn days(&self) -> i64 {
        days_from_civil(self.year, self.month, self.day)
    }

    /// 0 = Sunday … 6 = Saturday.
    pub fn weekday(&self) -> u32 {
        (self.days() + 4).rem_euclid(7) as u32
    }

    /// 1-based.
    pub fn day_of_year(&self) -> u32 {
        (self.days() - days_from_civil(self.year, 1, 1)) as u32 + 1
    }

    /// The same time `n` days later (or earlier).
    #[cfg(test)]
    pub fn add_days(&self, n: i64) -> DateTime {
        let (year, month, day) = civil_from_days(self.days() + n);
        DateTime { year, month, day, ..*self }
    }

    /// Seconds since 1970-01-01 UTC.
    pub fn unix(&self) -> i64 {
        self.days() * 86_400 + i64::from(self.hour * 3600 + self.minute * 60 + self.second) - i64::from(self.offset) * 60
    }

    /// (week-year, week) with weeks starting on `dow` (0 = Sunday) and week 1
    /// holding January `7 + dow - doy`: moment's rule (ISO: 1, 4; English: 0, 6).
    fn week(&self, dow: u32, doy: u32) -> (i32, u32) {
        let first_offset = |y: i32| -> i64 {
            let fwd = 7 + i64::from(dow) - i64::from(doy);
            let fwd_wd = (days_from_civil(y, 1, fwd as u32) + 4).rem_euclid(7);
            let fwdlw = (7 + fwd_wd - i64::from(dow)).rem_euclid(7);
            -fwdlw + fwd - 1
        };
        let weeks_in = |y: i32| -> i64 { (if leap(y) { 366 } else { 365 } - first_offset(y) + first_offset(y + 1)) / 7 };
        let week = (i64::from(self.day_of_year()) - first_offset(self.year) - 1).div_euclid(7) + 1;
        if week < 1 {
            (self.year - 1, (week + weeks_in(self.year - 1)) as u32)
        } else if week > weeks_in(self.year) {
            (self.year + 1, (week - weeks_in(self.year)) as u32)
        } else {
            (self.year, week as u32)
        }
    }
}

#[cfg(unix)]
fn local_now() -> DateTime {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    let secs = now.as_secs() as libc::time_t;
    // SAFETY: localtime_r only writes the `tm` it is given.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let ok = unsafe { !libc::localtime_r(&secs, &mut tm).is_null() };
    if !ok {
        return utc(now);
    }
    DateTime {
        year: tm.tm_year + 1900,
        month: (tm.tm_mon + 1) as u32,
        day: tm.tm_mday as u32,
        hour: tm.tm_hour as u32,
        minute: tm.tm_min as u32,
        second: tm.tm_sec.min(59) as u32,
        millis: now.subsec_millis(),
        offset: (tm.tm_gmtoff / 60) as i32,
    }
}

#[cfg(windows)]
fn local_now() -> DateTime {
    use windows_sys::Win32::Foundation::SYSTEMTIME;
    use windows_sys::Win32::System::SystemInformation::{GetLocalTime, GetSystemTime};
    let (mut local, mut utc_now): (SYSTEMTIME, SYSTEMTIME) = (unsafe { std::mem::zeroed() }, unsafe { std::mem::zeroed() });
    // SAFETY: both only fill in the struct they are given.
    unsafe {
        GetLocalTime(&mut local);
        GetSystemTime(&mut utc_now);
    }
    let minutes = |t: &SYSTEMTIME| days_from_civil(i32::from(t.wYear), u32::from(t.wMonth), u32::from(t.wDay)) * 1440 + i64::from(t.wHour) * 60 + i64::from(t.wMinute);
    DateTime {
        year: i32::from(local.wYear),
        month: u32::from(local.wMonth),
        day: u32::from(local.wDay),
        hour: u32::from(local.wHour),
        minute: u32::from(local.wMinute),
        second: u32::from(local.wSecond),
        millis: u32::from(local.wMilliseconds),
        offset: (minutes(&local) - minutes(&utc_now)) as i32,
    }
}

#[cfg(not(any(unix, windows)))]
fn local_now() -> DateTime {
    utc(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default())
}

#[cfg_attr(windows, allow(dead_code))]
fn utc(since_epoch: std::time::Duration) -> DateTime {
    let secs = since_epoch.as_secs() as i64;
    let (year, month, day) = civil_from_days(secs.div_euclid(86_400));
    let s = secs.rem_euclid(86_400) as u32;
    DateTime { year, month, day, hour: s / 3600, minute: s / 60 % 60, second: s % 60, millis: since_epoch.subsec_millis(), offset: 0 }
}

fn ordinal(n: u32) -> String {
    let suffix = match (n % 10, n % 100) {
        (_, 11..=13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    };
    format!("{n}{suffix}")
}

/// moment.js tokens, longest first so `YYYY` wins over `YY`.
const TOKENS: &[&str] = &[
    "YYYY", "gggg", "GGGG", "MMMM", "dddd", "DDDD", "DDDo", "LLLL", "llll", "MMM", "ddd", "DDD", "SSS", "LTS", "LLL", "lll", "YY", "gg", "GG", "MM", "Mo", "DD", "Do", "dd", "do", "ww", "wo", "WW", "Wo", "Qo", "HH", "hh", "kk", "mm", "ss", "SS", "ZZ", "LT", "LL", "ll", "Y", "Q", "M", "D", "d", "e", "E", "w", "W", "A", "a", "H", "h", "k", "m", "s", "S", "Z", "X", "x", "L", "l",
];

/// `t` in a moment.js format. Text in `[brackets]` is kept as written, as
/// is anything that isn't a token.
pub fn format(fmt: &str, t: &DateTime) -> String {
    let mut out = String::new();
    let mut rest = fmt;
    while let Some(c) = rest.chars().next() {
        if c == '[' {
            match rest[1..].find(']') {
                Some(end) => {
                    out.push_str(&rest[1..1 + end]);
                    rest = &rest[end + 2..];
                }
                None => {
                    out.push_str(&rest[1..]);
                    rest = "";
                }
            }
            continue;
        }
        match TOKENS.iter().find(|tok| rest.starts_with(**tok)) {
            Some(tok) => {
                out.push_str(&token(tok, t));
                rest = &rest[tok.len()..];
            }
            None => {
                out.push(c);
                rest = &rest[c.len_utf8()..];
            }
        }
    }
    out
}

fn token(tok: &str, t: &DateTime) -> String {
    let h12 = if t.hour.is_multiple_of(12) { 12 } else { t.hour % 12 };
    let wd = t.weekday();
    let (iso_year, iso_week) = t.week(1, 4);
    let (loc_year, loc_week) = t.week(0, 6);
    let quarter = (t.month - 1) / 3 + 1;
    let zone = |sep: &str| {
        let sign = if t.offset < 0 { '-' } else { '+' };
        let m = t.offset.unsigned_abs();
        format!("{sign}{:02}{sep}{:02}", m / 60, m % 60)
    };
    match tok {
        "YYYY" => format!("{:04}", t.year),
        "YY" => format!("{:02}", t.year.rem_euclid(100)),
        "Y" => t.year.to_string(),
        "Q" => quarter.to_string(),
        "Qo" => ordinal(quarter),
        "MMMM" => MONTHS[(t.month - 1) as usize].to_owned(),
        "MMM" => MONTHS[(t.month - 1) as usize][..3].to_owned(),
        "MM" => format!("{:02}", t.month),
        "Mo" => ordinal(t.month),
        "M" => t.month.to_string(),
        "DDDD" => format!("{:03}", t.day_of_year()),
        "DDDo" => ordinal(t.day_of_year()),
        "DDD" => t.day_of_year().to_string(),
        "DD" => format!("{:02}", t.day),
        "Do" => ordinal(t.day),
        "D" => t.day.to_string(),
        "dddd" => DAYS[wd as usize].to_owned(),
        "ddd" => DAYS[wd as usize][..3].to_owned(),
        "dd" => DAYS[wd as usize][..2].to_owned(),
        "do" => ordinal(wd),
        "d" | "e" => wd.to_string(),
        "E" => (if wd == 0 { 7 } else { wd }).to_string(),
        "ww" => format!("{loc_week:02}"),
        "wo" => ordinal(loc_week),
        "w" => loc_week.to_string(),
        "WW" => format!("{iso_week:02}"),
        "Wo" => ordinal(iso_week),
        "W" => iso_week.to_string(),
        "gggg" => format!("{loc_year:04}"),
        "gg" => format!("{:02}", loc_year.rem_euclid(100)),
        "GGGG" => format!("{iso_year:04}"),
        "GG" => format!("{:02}", iso_year.rem_euclid(100)),
        "A" => (if t.hour < 12 { "AM" } else { "PM" }).to_owned(),
        "a" => (if t.hour < 12 { "am" } else { "pm" }).to_owned(),
        "HH" => format!("{:02}", t.hour),
        "H" => t.hour.to_string(),
        "hh" => format!("{h12:02}"),
        "h" => h12.to_string(),
        "kk" => format!("{:02}", if t.hour == 0 { 24 } else { t.hour }),
        "k" => (if t.hour == 0 { 24 } else { t.hour }).to_string(),
        "mm" => format!("{:02}", t.minute),
        "m" => t.minute.to_string(),
        "ss" => format!("{:02}", t.second),
        "s" => t.second.to_string(),
        "SSS" => format!("{:03}", t.millis),
        "SS" => format!("{:02}", t.millis / 10),
        "S" => (t.millis / 100).to_string(),
        "ZZ" => zone(""),
        "Z" => zone(":"),
        "X" => t.unix().to_string(),
        "x" => (t.unix() * 1000 + i64::from(t.millis)).to_string(),
        "LT" => format("h:mm A", t),
        "LTS" => format("h:mm:ss A", t),
        "L" => format("MM/DD/YYYY", t),
        "LL" => format("MMMM D, YYYY", t),
        "LLL" => format("MMMM D, YYYY h:mm A", t),
        "LLLL" => format("dddd, MMMM D, YYYY h:mm A", t),
        "l" => format("M/D/YYYY", t),
        "ll" => format("MMM D, YYYY", t),
        "lll" => format("MMM D, YYYY h:mm A", t),
        "llll" => format("ddd, MMM D, YYYY h:mm A", t),
        _ => tok.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime {
        DateTime { hour: h, minute: mi, second: 7, millis: 45, ..DateTime::date(y, mo, d) }
    }

    #[test]
    fn obsidian_daily_note_formats() {
        let t = at(2026, 10, 19, 9, 5);
        assert_eq!(format("YYYY-MM-DD", &t), "2026-10-19");
        assert_eq!(format("YYYY/MM/YYYY-MM-DD", &t), "2026/10/2026-10-19");
        assert_eq!(format("ddd, MMM D YY", &t), "Mon, Oct 19 26");
        assert_eq!(format("dddd, MMMM Do YYYY", &t), "Monday, October 19th 2026");
        assert_eq!(format("[Week] W [of] GGGG", &t), "Week 43 of 2026");
        assert_eq!(format("HH:mm:ss.SSS A h a", &t), "09:05:07.045 AM 9 am");
        assert_eq!(format("DDDD DDD Q Qo E d dd", &t), "292 292 4 4th 1 1 Mo");
        assert_eq!(format("[YYYY] [literal]: é ü", &t), "YYYY literal: é ü");
        assert_eq!(format("L LT", &t), "10/19/2026 9:05 AM");
        let noon = at(2026, 1, 1, 12, 0);
        assert_eq!(format("hh A k", &noon), "12 PM 12");
        assert_eq!(format("hh k", &at(2026, 1, 1, 0, 0)), "12 24");
    }

    #[test]
    fn ordinals_weekdays_and_weeks() {
        assert_eq!([1, 2, 3, 4, 11, 12, 13, 21, 22, 23, 101, 111].map(ordinal), ["1st", "2nd", "3rd", "4th", "11th", "12th", "13th", "21st", "22nd", "23rd", "101st", "111th"]);
        assert_eq!(DateTime::date(1970, 1, 1).weekday(), 4, "a Thursday");
        assert_eq!(DateTime::date(2000, 2, 29).weekday(), 2);
        // ISO weeks around new year: 2021-01-03 is in 2020's week 53; 2024-12-30 is 2025-W01.
        assert_eq!(format("GGGG-[W]WW", &DateTime::date(2021, 1, 3)), "2020-W53");
        assert_eq!(format("GGGG-[W]WW", &DateTime::date(2024, 12, 30)), "2025-W01");
        // moment's English weeks start on Sunday; the week with January 1st is week 1.
        assert_eq!(format("gggg w", &DateTime::date(2022, 1, 1)), "2022 1");
        assert_eq!(format("gggg w", &DateTime::date(2021, 12, 26)), "2022 1");
        assert_eq!(format("gggg ww", &DateTime::date(2021, 12, 25)), "2021 52");
    }

    #[test]
    fn civil_dates_round_trip_and_zones() {
        for days in [-1000i64, 0, 59, 60, 11_016, 20_744, 100_000] {
            let (y, m, d) = civil_from_days(days);
            assert_eq!(days_from_civil(y, m, d), days);
        }
        assert_eq!(DateTime::date(2024, 2, 28).add_days(1).day, 29);
        assert_eq!(DateTime::date(2023, 12, 31).add_days(1), DateTime::date(2024, 1, 1));
        let t = DateTime { offset: -330, ..DateTime::date(1970, 1, 1) };
        assert_eq!(format("Z ZZ X", &t), "-05:30 -0530 19800");
        let now = DateTime::now();
        assert!((1..=12).contains(&now.month) && (1..=31).contains(&now.day) && now.year >= 2024);
        assert!(now.offset.abs() <= 14 * 60);
    }
}
