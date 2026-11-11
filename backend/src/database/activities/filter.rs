use diesel::dsl::exists;
use diesel::pg::Pg;
use diesel::prelude::*;

use crate::models::Filter;
use crate::schema::{activities, track_points};

impl Filter {
    pub(super) fn apply<'a, ST: 'a>(
        &self,
        mut query: activities::BoxedQuery<'a, Pg, ST>,
    ) -> activities::BoxedQuery<'a, Pg, ST> {
        query = query.filter(activities::user_id.eq(self.user_id));
        if !self.without_gps {
            query = query.filter(exists(
                track_points::table.filter(track_points::activity_id.eq(activities::id)),
            ));
        }
        if let Some(id) = self.id {
            query = query.filter(activities::id.eq(id));
        }
        for term in search_terms(self.query.as_deref().unwrap_or_default()) {
            let text = activities::search_text.ilike(contains(&term.text));
            query = match term.amount {
                Some(Amount::Meters(meters)) => {
                    let (low, high) = around(meters);
                    query.filter(text.or(activities::distance_m.between(low, high)))
                }
                Some(Amount::Seconds(seconds)) => {
                    let (low, high) = around(seconds);
                    query.filter(text.or(activities::duration_s.between(low, high)))
                }
                None => query.filter(text),
            };
        }
        if !self.activity_types.is_empty() {
            query = query.filter(activities::activity_type.eq_any(self.activity_types.clone()));
        }
        if let Some(title) = &self.title {
            query = query.filter(activities::name.ilike(contains(title)));
        }
        if let Some(description) = &self.description {
            query = query.filter(activities::description.ilike(contains(description)));
        }
        if let Some(location) = &self.location {
            query = query.filter(activities::location.ilike(contains(location)));
        }
        if let Some(min) = self.min_distance_m {
            query = query.filter(activities::distance_m.ge(min));
        }
        if let Some(max) = self.max_distance_m {
            query = query.filter(activities::distance_m.le(max));
        }
        if let Some(min) = self.min_duration_s {
            query = query.filter(activities::duration_s.ge(min));
        }
        if let Some(max) = self.max_duration_s {
            query = query.filter(activities::duration_s.le(max));
        }
        if let Some(from) = self.from {
            query = query.filter(activities::started_local.ge(from.to_string()));
        }
        // A prefix sorts before all times on its day, so this needs the next day.
        if let Some(next_day) = self.to.and_then(|to| to.succ_opt()) {
            query = query.filter(activities::started_local.lt(next_day.to_string()));
        }
        query
    }
}

/// How far a searched length may be off, since no 10 km run measures exactly 10 km.
const AMOUNT_TOLERANCE: f64 = 0.05;

fn around(amount: f64) -> (f64, f64) {
    (
        amount * (1.0 - AMOUNT_TOLERANCE),
        amount * (1.0 + AMOUNT_TOLERANCE),
    )
}

#[derive(Debug, PartialEq)]
enum Amount {
    Meters(f64),
    Seconds(f64),
}

#[derive(Debug, PartialEq)]
struct SearchTerm {
    text: String,
    amount: Option<Amount>,
}

/// Splits a search into words that must all match. A word like `10km` or `45min`, or
/// `10 km` with a space, also matches activities of about that length.
fn search_terms(search: &str) -> Vec<SearchTerm> {
    let mut words = search.split_whitespace().peekable();
    let mut terms = Vec::new();
    while let Some(word) = words.next() {
        let spaced_amount = words
            .peek()
            .and_then(|unit| parse_amount(&format!("{word}{unit}")));
        if word.parse::<f64>().is_ok()
            && let Some(amount) = spaced_amount
            && let Some(unit) = words.next()
        {
            terms.push(SearchTerm {
                text: format!("{word} {unit}"),
                amount: Some(amount),
            });
        } else {
            terms.push(SearchTerm {
                text: word.to_owned(),
                amount: parse_amount(word),
            });
        }
    }
    terms
}

fn parse_amount(word: &str) -> Option<Amount> {
    let word = word.to_lowercase();
    let split = word.find(|c: char| !c.is_ascii_digit() && c != '.')?;
    let (number, unit) = word.split_at(split);
    let number: f64 = number.parse().ok()?;
    match unit {
        "km" | "k" => Some(Amount::Meters(number * 1000.0)),
        "m" => Some(Amount::Meters(number)),
        "mi" => Some(Amount::Meters(number * 1609.344)),
        "min" => Some(Amount::Seconds(number * 60.0)),
        "h" => Some(Amount::Seconds(number * 3600.0)),
        _ => None,
    }
}

/// A LIKE pattern that finds the text anywhere, with its wildcards taken literally.
fn contains(text: &str) -> String {
    let escaped = text
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_");
    format!("%{escaped}%")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn term(text: &str, amount: Option<Amount>) -> SearchTerm {
        SearchTerm {
            text: text.to_owned(),
            amount,
        }
    }

    #[test]
    fn search_finds_lengths_with_and_without_a_space() {
        assert_eq!(
            search_terms("park 10km 5 k 45min 2 parks"),
            [
                term("park", None),
                term("10km", Some(Amount::Meters(10_000.0))),
                term("5 k", Some(Amount::Meters(5_000.0))),
                term("45min", Some(Amount::Seconds(2_700.0))),
                term("2", None),
                term("parks", None),
            ]
        );
    }

    #[test]
    fn words_that_only_look_like_lengths_stay_text() {
        assert_eq!(parse_amount("k"), None);
        assert_eq!(parse_amount("10"), None);
        assert_eq!(parse_amount("1.2.3km"), None);
        assert_eq!(parse_amount("5kg"), None);
    }

    #[test]
    fn patterns_take_wildcards_literally() {
        assert_eq!(contains(r"50%_a\b"), r"%50\%\_a\\b%");
    }
}
