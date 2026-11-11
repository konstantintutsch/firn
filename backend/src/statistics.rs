//! Calculations for the statistics page.

use chrono::{Days, NaiveDate};

/// For each day with a value, the average of the values on it and the `days - 1` days before.
pub fn rolling_average(values: &[(NaiveDate, f64)], days: u64) -> Vec<(NaiveDate, f64)> {
    let mut start = 0;
    let mut total = 0.0;
    values
        .iter()
        .enumerate()
        .map(|(end, &(day, value))| {
            total += value;
            while values[start].0 + Days::new(days) <= day {
                total -= values[start].1;
                start += 1;
            }
            (day, total / (end + 1 - start) as f64)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rolling_average_skips_days_out_of_the_window() {
        let day = |text: &str| text.parse::<NaiveDate>().unwrap();
        let values = [
            (day("2026-01-01"), 50.0),
            (day("2026-01-02"), 48.0),
            (day("2026-01-07"), 46.0),
            (day("2026-01-08"), 40.0),
        ];
        assert_eq!(
            rolling_average(&values, 7),
            [
                (day("2026-01-01"), 50.0),
                (day("2026-01-02"), 49.0),
                (day("2026-01-07"), 48.0),
                (day("2026-01-08"), 44.666666666666664),
            ]
        );
    }
}
