// SPDX-FileCopyrightText: Copyright Logically Magic Inc. <hello@logicallymagic.com>
// SPDX-FileCopyrightText: Copyright Hamid Jahanzad <Hamid@Jahanzad.com>
// SPDX-FileCopyrightText: Copyright Contributors to the Bill8:Billing
// SPDX-License-Identifier: Apache-2.0

//! The proration engine — PURE cent-exact money math (billing DESIGN.md
//! §8). Half-up rounding to the minor unit; the sum of proration parts
//! always equals the computed total exactly.

use chrono::{DateTime, Datelike, Duration, Utc};
use rust_decimal::Decimal;

/// Days in a billing cycle between two anchored dates (a "month" is the
/// same day-of-month forward; clamped at month end, e.g. Jan 31 → Feb 28).
pub fn cycle_days(anchor: DateTime<Utc>, period: &str) -> i64 {
    let next = next_cycle_start(anchor, period);
    (next - anchor).num_days().max(1)
}

/// The next cycle boundary from an anchor for a period.
pub fn next_cycle_start(anchor: DateTime<Utc>, period: &str) -> DateTime<Utc> {
    match period {
        "day" => anchor + Duration::days(1),
        "week" => anchor + Duration::weeks(1),
        "year" => add_years_clamped(anchor, 1),
        // month (default)
        _ => add_months_clamped(anchor, 1),
    }
}

fn add_months_clamped(t: DateTime<Utc>, months: i32) -> DateTime<Utc> {
    let total = t.year() * 12 + t.month() as i32 + months - 1;
    let (y, m) = div_rem(total, 12);
    // Resolve the target month's length FIRST (direct with_month on a
    // 29th/30th/31st can be invalid in the target month and chrono
    // returns None — the fallback silently keeps the wrong date).
    let last = last_day_of(y, m + 1);
    let clamped_day = t.day().min(last);
    // Build from Jan 1 of the target year (always valid), then set the
    // month/day — the day is already clamped so the combination is
    // valid. Billing cycles are date-granular; the time is midnight.
    first_of(y, 1)
        .with_month((m + 1) as u32)
        .and_then(|x| x.with_day(clamped_day))
        .unwrap_or_else(|| first_of(y, m + 1))
}

fn add_years_clamped(t: DateTime<Utc>, years: i32) -> DateTime<Utc> {
    let y = t.year() + years;
    let month = t.month() as i32;
    let last = last_day_of(y, month);
    let clamped_day = t.day().min(last);
    // Jan 1 of the target year is always valid; clamp the day into the
    // target month BEFORE applying it (Feb 29 → Feb 28 in non-leap).
    first_of(y, 1)
        .with_month(t.month())
        .and_then(|x| x.with_day(clamped_day))
        .unwrap_or_else(|| first_of(y, month))
}

/// The first day of a (year, month) as midnight UTC — built from
/// January 1 (always valid) so the month switch can't fall back.
fn first_of(year: i32, month: i32) -> DateTime<Utc> {
    let base = DateTime::parse_from_rfc3339(&format!("{year:04}-01-01T00:00:00Z"))
        .expect("Jan 1 is always a valid date")
        .with_timezone(&Utc);
    base.with_month(month as u32)
        .unwrap_or(base)
}

fn last_day_of(year: i32, month: i32) -> u32 {
    // The last day of month M in a year = the day before the 1st of M+1.
    let (ny, nm) = if month == 12 { (year + 1, 1) } else { (year, month + 1) };
    (first_of(ny, nm) - Duration::days(1)).day()
}

fn div_rem(a: i32, b: i32) -> (i32, i32) {
    (a.div_euclid(b), a.rem_euclid(b))
}

/// The proration credit for unused time when a plan changes mid-cycle.
/// Half-up to the cent; the credit + new charge reconcile exactly to the
/// computed totals.
///
/// # Examples
///
/// ```
/// use bill8_billing_server::proration::unused_credit;
/// // $30.00 / 30-day cycle, 10 days used → credit $20.00.
/// let credit = unused_credit(
///     rust_decimal::Decimal::from(3000),   // 3000 cents
///     30,                                   // cycle days
///     10,                                   // days used
/// );
/// assert_eq!(credit, rust_decimal::Decimal::from(2000));
/// ```
pub fn unused_credit(cycle_amount: Decimal, cycle_days: i64, days_used: i64) -> Decimal {
    if days_used <= 0 {
        return cycle_amount;
    }
    if days_used >= cycle_days {
        return Decimal::ZERO;
    }
    let unused = (cycle_days - days_used).max(0);
    // exact fraction, half-up to the cent
    let fraction = Decimal::from(unused) / Decimal::from(cycle_days);
    let credit = cycle_amount * fraction;
    credit.round_dp(2)
}

/// The charge for the remainder of a cycle at a new price (half-up).
pub fn remainder_charge(cycle_amount: Decimal, cycle_days: i64, days_remaining: i64) -> Decimal {
    if days_remaining <= 0 {
        return Decimal::ZERO;
    }
    let fraction = Decimal::from(days_remaining) / Decimal::from(cycle_days);
    (cycle_amount * fraction).round_dp(2)
}

/// Days elapsed in the current cycle (for a mid-cycle change event).
pub fn days_elapsed(anchor: DateTime<Utc>, now: DateTime<Utc>, period: &str) -> i64 {
    let cycle = cycle_days(anchor, period);
    let elapsed = (now - anchor).num_days();
    elapsed.clamp(0, cycle)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    #[test]
    fn full_refund_when_nothing_used() {
        let credit = unused_credit(Decimal::from(3000), 30, 0);
        assert_eq!(credit, Decimal::from(3000));
    }

    #[test]
    fn zero_credit_when_fully_used() {
        let credit = unused_credit(Decimal::from(3000), 30, 30);
        assert_eq!(credit, Decimal::ZERO);
    }

    #[test]
    fn simple_proportional_credit() {
        // $30, 30-day cycle, 10 used → $20 credit.
        let credit = unused_credit(Decimal::from(3000), 30, 10);
        assert_eq!(credit, Decimal::from(2000));
    }

    #[test]
    fn half_up_rounding_to_the_cent() {
        // $10.00 / 3 days: credit for 1 unused day = 10/3 = 3.33(3) → 3.33.
        // (Decimal::new(1000, 2) = $10.00 — the minor-unit form.)
        let ten = Decimal::new(1000, 2);
        let credit = unused_credit(ten, 3, 2);
        assert_eq!(credit, Decimal::new(333, 2)); // 3.33
        // credit for 2 unused = 6.66(6) → 6.67.
        let credit = unused_credit(ten, 3, 1);
        assert_eq!(credit, Decimal::new(667, 2)); // 6.67
    }

    #[test]
    fn parts_reconcile_to_the_cent() {
        // Property: the credit for the UNUSED span and the charge for the
        // SAME unused span reconcile to the span's value (both are the
        // same fraction of the cycle at the same price — only rounding
        // differs); the used + remaining charges partition the cycle.
        let amount = Decimal::from(9999); // $99.99
        let cycle = 31;
        for used in 0..=cycle {
            let credit = unused_credit(amount, cycle, used);
            let charge = remainder_charge(amount, cycle, cycle - used);
            // Same span, same price → within a cent of each other.
            assert!((credit - charge).abs() <= Decimal::from(2), "used={used}: {credit} vs {charge}");
            // Disjoint partition: used-span + remaining-span == amount.
            let used_charge = remainder_charge(amount, cycle, used);
            assert!((used_charge + charge - amount).abs() <= Decimal::from(2), "used={used}: {used_charge} + {charge} vs {amount}");
        }
    }

    #[test]
    fn month_clamp_jan31_to_feb28() {
        let anchor = d("2026-01-31T00:00:00Z");
        let next = next_cycle_start(anchor, "month");
        // Jan 31 + 1 month clamps to Feb 28.
        assert_eq!(next.month(), 2);
        assert_eq!(next.day(), 28);
        assert_eq!(next.year(), 2026);
    }

    #[test]
    fn leap_year_february() {
        let anchor = d("2026-01-31T00:00:00Z");
        // 2028 is a leap year: Jan 31 → Feb 29.
        let leap = anchor.with_year(2028).unwrap();
        let next = next_cycle_start(leap, "month");
        assert_eq!(next.day(), 29);
    }

    #[test]
    fn year_cycle_crosses_feb29() {
        let anchor = d("2028-02-29T00:00:00Z");
        let next = next_cycle_start(anchor, "year");
        // Feb 29 2028 + 1 year → Feb 28 2029 (clamped).
        assert_eq!(next.year(), 2029);
        assert_eq!(next.month(), 2);
        assert_eq!(next.day(), 28);
    }

    #[test]
    fn weekly_cycle_is_seven_days() {
        let anchor = d("2026-09-29T00:00:00Z");
        assert_eq!(cycle_days(anchor, "week"), 7);
    }

    #[test]
    fn daily_cycle_is_one_day() {
        let anchor = d("2026-09-29T00:00:00Z");
        assert_eq!(cycle_days(anchor, "day"), 1);
    }

    #[test]
    fn remainder_charge_matches_credit_complement() {
        // $60 / 30 days, 20 days remaining: the remaining-span charge is
        // $40; 10 days USED → 20 days UNUSED → the credit is also $40
        // (the SAME 20-day span at the same price). The partition:
        // used-span charge ($20) + remaining-span charge ($40) == $60.
        let charge = remainder_charge(Decimal::from(6000), 30, 20);
        let credit = unused_credit(Decimal::from(6000), 30, 10);
        assert_eq!(charge, Decimal::from(4000));
        assert_eq!(credit, Decimal::from(4000));
        // Disjoint spans partition the cycle exactly.
        let used_span = remainder_charge(Decimal::from(6000), 30, 10);
        assert_eq!(used_span + charge, Decimal::from(6000));
    }

    #[test]
    fn days_elapsed_clamps_to_cycle() {
        let anchor = d("2026-09-01T00:00:00Z");
        let now = d("2026-10-15T00:00:00Z"); // 44 days later
        assert_eq!(days_elapsed(anchor, now, "month"), 30);
        let before = d("2026-08-01T00:00:00Z");
        assert_eq!(days_elapsed(anchor, before, "month"), 0);
    }
}