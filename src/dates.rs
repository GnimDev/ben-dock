//! Calendar arithmetic for the date panel. Months are 1..=12; weekdays are
//! 0 = Monday .. 6 = Sunday (the week starts on Monday).

pub const WEEKDAYS_SHORT: [&str; 7] = ["T2", "T3", "T4", "T5", "T6", "T7", "CN"];

pub fn is_leap(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

pub fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        4 | 6 | 9 | 11 => 30,
        2 => {
            if is_leap(year) {
                29
            } else {
                28
            }
        }
        _ => 31,
    }
}

/// Day of the week of a date: 0 = Monday .. 6 = Sunday.
pub fn weekday(year: i32, month: u32, day: u32) -> u32 {
    // Sakamoto's method gives 0 = Sunday.
    const T: [i32; 12] = [0, 3, 2, 5, 0, 3, 5, 1, 4, 6, 2, 4];
    let m = month.clamp(1, 12) as usize;
    let y = if m < 3 { year - 1 } else { year };
    let sunday_first = (y + y.div_euclid(4) - y.div_euclid(100) + y.div_euclid(400) + T[m - 1] + day as i32).rem_euclid(7);
    ((sunday_first + 6) % 7) as u32
}

/// The month `by` months after (or before) the given one.
pub fn shift_month(year: i32, month: u32, by: i32) -> (i32, u32) {
    let index = year * 12 + month as i32 - 1 + by;
    (index.div_euclid(12), index.rem_euclid(12) as u32 + 1)
}

/// How many week rows a month needs when laid out Monday-first.
pub fn week_rows(year: i32, month: u32) -> u32 {
    (weekday(year, month, 1) + days_in_month(year, month)).div_ceil(7)
}

/// Column (0 = Monday) and row of a day in its month's grid.
pub fn cell(year: i32, month: u32, day: u32) -> (u32, u32) {
    let index = weekday(year, month, 1) + day - 1;
    (index % 7, index / 7)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_weekdays() {
        assert_eq!(weekday(2026, 11, 15), 6); // a Sunday
        assert_eq!(weekday(2024, 2, 29), 3); // a Thursday
        assert_eq!(weekday(2000, 1, 1), 5); // a Saturday
        assert_eq!(weekday(2026, 1, 1), 3); // a Thursday
        assert_eq!(weekday(1999, 12, 31), 4); // a Friday
    }

    #[test]
    fn month_lengths() {
        assert_eq!(days_in_month(2024, 2), 29);
        assert_eq!(days_in_month(2100, 2), 28);
        assert_eq!(days_in_month(2000, 2), 29);
        assert_eq!(days_in_month(2026, 11), 30);
        assert_eq!(days_in_month(2026, 12), 31);
    }

    #[test]
    fn months_wrap_around_years() {
        assert_eq!(shift_month(2026, 1, -1), (2025, 12));
        assert_eq!(shift_month(2026, 12, 1), (2027, 1));
        assert_eq!(shift_month(2026, 11, -23), (2024, 12));
        assert_eq!(shift_month(2026, 11, 0), (2026, 11));
    }

    #[test]
    fn grid_holds_every_day() {
        for year in [2023, 2024, 2026, 2027] {
            for month in 1..=12 {
                let rows = week_rows(year, month);
                assert!((4..=6).contains(&rows));
                let (col, row) = cell(year, month, days_in_month(year, month));
                assert!(col < 7 && row == rows - 1, "{year}-{month}");
                assert_eq!(cell(year, month, 1).1, 0);
            }
        }
        // February 2027 starts on a Monday and has 28 days: exactly four rows.
        assert_eq!(week_rows(2027, 2), 4);
        // November 2026 starts on a Sunday: the 1st is alone in the first row.
        assert_eq!(cell(2026, 11, 1), (6, 0));
        assert_eq!(cell(2026, 11, 15), (6, 2));
    }
}
