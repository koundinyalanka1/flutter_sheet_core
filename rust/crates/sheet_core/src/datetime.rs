//! Excel date serials.
//!
//! A date is a count of days, a time the fraction of one. In the 1900 date
//! system serial 1 is 1900-01-01, and, as in Lotus 1-2-3, serial 60 is the
//! 29th of February 1900, a day that never existed. In the 1904 system
//! serial 0 is 1904-01-01.

/// The last serial Excel accepts as a date: 9999-12-31.
pub const MAX_DATE_SERIAL: f64 = 2_958_465.0;

/// Days since 1970-01-01 of a proleptic Gregorian date (Howard Hinnant's
/// algorithm). Day 0 of a month is the last day of the previous one.
pub fn days_from_civil(year: i64, month: u32, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let m = month as i64;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468 + (day - 1)
}

/// The date `days` after 1970-01-01.
pub fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + if month <= 2 { 1 } else { 0 };
    (year, month, day)
}

const DAYS_1899_12_31: i64 = -25_568;
const DAYS_1904_01_01: i64 = -24_107;

/// The serial of a date, normalizing out-of-range months and days the way
/// DATE does (month 13 is January of the next year, day 0 the last day of
/// the previous month). Years below 1900 are taken as 1900 + year. None
/// when the result is before the date system's first day or after 9999.
pub fn date_to_serial(year: i64, month: i64, day: i64, date1904: bool) -> Option<f64> {
    let year = if (0..1900).contains(&year) && !date1904 {
        year + 1900
    } else if (0..1904).contains(&year) && date1904 {
        year + 1900
    } else {
        year
    };
    if !(0..=9999).contains(&year) {
        return None;
    }
    // Normalize the month into the year.
    let months = year * 12 + (month - 1);
    let (y, m) = (months.div_euclid(12), months.rem_euclid(12) as u32 + 1);
    if !date1904 && y == 1900 && m == 2 && day == 29 {
        return Some(60.0);
    }
    let days = days_from_civil(y, m, day);
    let serial = if date1904 {
        days - DAYS_1904_01_01
    } else {
        let s = days - DAYS_1899_12_31;
        if s >= 60 {
            s + 1
        } else {
            s
        }
    };
    (serial >= 0 && serial as f64 <= MAX_DATE_SERIAL).then_some(serial as f64)
}

/// The calendar date of a serial (its whole part). None outside
/// 0..=9999-12-31. Serial 0 in the 1900 system is "1900-01-00", as Excel
/// shows it.
pub fn serial_to_date(serial: f64, date1904: bool) -> Option<(i64, u32, u32)> {
    if !serial.is_finite() || serial < 0.0 {
        return None;
    }
    let whole = serial.floor() as i64;
    if date1904 {
        if whole as f64 > MAX_DATE_SERIAL - 1462.0 {
            return None;
        }
        return Some(civil_from_days(DAYS_1904_01_01 + whole));
    }
    if whole as f64 > MAX_DATE_SERIAL {
        return None;
    }
    match whole {
        0 => Some((1900, 1, 0)),
        60 => Some((1900, 2, 29)),
        s if s < 60 => Some(civil_from_days(DAYS_1899_12_31 + s)),
        s => Some(civil_from_days(DAYS_1899_12_31 + s - 1)),
    }
}

/// Hours, minutes, seconds and milliseconds of a serial's time of day,
/// rounded to the millisecond as Excel displays them.
pub fn time_of_day(serial: f64) -> (u32, u32, u32, u32) {
    let fraction = serial - serial.floor();
    let mut ms = (fraction * 86_400_000.0).round() as u64;
    if ms >= 86_400_000 {
        ms = 0;
    }
    let h = ms / 3_600_000;
    let m = (ms / 60_000) % 60;
    let s = (ms / 1000) % 60;
    (h as u32, m as u32, s as u32, (ms % 1000) as u32)
}

/// Day of the week, 1 = Sunday .. 7 = Saturday (WEEKDAY's default).
pub fn weekday(serial: f64, date1904: bool) -> Option<u32> {
    let (y, m, d) = serial_to_date(serial, date1904)?;
    if !date1904 && serial.floor() < 61.0 {
        // Excel counts 1900-01-01 as a Sunday (it was a Monday), because of
        // the phantom leap day; keep its answer.
        return Some(((serial.floor() as i64 - 1).rem_euclid(7) + 1) as u32);
    }
    let days = days_from_civil(y, m, d as i64);
    // 1970-01-01 was a Thursday (5).
    Some(((days + 4).rem_euclid(7) + 1) as u32)
}

pub fn is_leap_year(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

pub fn days_in_month(year: i64, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(year) => 29,
        _ => 28,
    }
}

pub const MONTH_NAMES: [&str; 12] = [
    "January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November",
    "December",
];

pub const DAY_NAMES: [&str; 7] = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_round_trip() {
        for days in [-1_000_000, -25_568, -1, 0, 1, 19_000, 2_932_896] {
            let (y, m, d) = civil_from_days(days);
            assert_eq!(days_from_civil(y, m, d as i64), days);
        }
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(days_from_civil(2000, 3, 1), 11_017);
    }

    #[test]
    fn excel_1900_serials() {
        let cases = [
            ((1900, 1, 1), 1.0),
            ((1900, 2, 28), 59.0),
            ((1900, 2, 29), 60.0),
            ((1900, 3, 1), 61.0),
            ((1999, 12, 31), 36_525.0),
            ((2024, 2, 29), 45_351.0),
            ((2026, 10, 13), 46_308.0),
            ((9999, 12, 31), 2_958_465.0),
        ];
        for ((y, m, d), serial) in cases {
            assert_eq!(date_to_serial(y, m, d, false), Some(serial), "{y}-{m}-{d}");
            assert_eq!(serial_to_date(serial, false), Some((y, m as u32, d as u32)), "{serial}");
        }
        assert_eq!(serial_to_date(0.0, false), Some((1900, 1, 0)));
        assert_eq!(serial_to_date(-1.0, false), None);
        assert_eq!(serial_to_date(2_958_466.0, false), None);
    }

    #[test]
    fn date_normalizes_like_excel() {
        // Month 13 rolls into the next year; day 0 is the last of the month before.
        assert_eq!(date_to_serial(2023, 13, 1, false), date_to_serial(2024, 1, 1, false));
        assert_eq!(date_to_serial(2024, 3, 0, false), date_to_serial(2024, 2, 29, false));
        assert_eq!(date_to_serial(2024, 0, 15, false), date_to_serial(2023, 12, 15, false));
        assert_eq!(date_to_serial(2024, 1, -1, false), date_to_serial(2023, 12, 30, false));
        // Two-digit-style years are offsets from 1900.
        assert_eq!(date_to_serial(99, 1, 1, false), date_to_serial(1999, 1, 1, false));
        assert_eq!(date_to_serial(10_000, 1, 1, false), None);
        assert_eq!(date_to_serial(1899, 12, 31, false), date_to_serial(3799, 12, 31, false));
    }

    #[test]
    fn excel_1904_serials() {
        assert_eq!(date_to_serial(1904, 1, 1, true), Some(0.0));
        assert_eq!(date_to_serial(2024, 2, 29, true), Some(45_351.0 - 1462.0));
        assert_eq!(serial_to_date(0.0, true), Some((1904, 1, 1)));
    }

    #[test]
    fn times_and_weekdays() {
        assert_eq!(time_of_day(0.5), (12, 0, 0, 0));
        assert_eq!(time_of_day(45_000.75), (18, 0, 0, 0));
        assert_eq!(time_of_day(1.0 / 86_400.0 * 3661.5), (1, 1, 1, 500));
        assert_eq!(time_of_day(0.999_999_999_9), (0, 0, 0, 0));
        // 2026-10-13 is a Tuesday.
        assert_eq!(weekday(46_308.0, false), Some(3));
        assert_eq!(weekday(1.0, false), Some(1));
        assert_eq!(days_in_month(2024, 2), 29);
        assert_eq!(days_in_month(1900, 2), 28);
    }
}
