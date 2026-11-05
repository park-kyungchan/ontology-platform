//! Publication-only arithmetic over already admitted typed report statements.
use crate::Output;
use crate::report::{AttributionInput, ExactFraction, MetricStatement};
use serde_json::{Value, json};

const ROUNDING: &str = "half_away_from_zero";
fn unavailable(reason: &str) -> Value {
    json!({"status":"not_available", "reason":reason, "native":null})
}
/// Decimal long division accepts the exact native integer string without a
/// machine-integer range limit. Each remainder operation is <= u64::MAX * 10.
fn decimal(n: &str, d: u64, places: usize) -> Option<String> {
    if d == 0 {
        return None;
    }
    let negative = n.starts_with('-');
    let digits = n
        .strip_prefix('-')
        .or_else(|| n.strip_prefix('+'))
        .unwrap_or(n);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let divisor = u128::from(d);
    let mut remainder = 0_u128;
    let mut rounded = Vec::with_capacity(digits.len() + places + 1);
    for digit in digits.bytes() {
        remainder = remainder * 10 + u128::from(digit - b'0');
        rounded.push((remainder / divisor) as u8);
        remainder %= divisor;
    }
    for _ in 0..places {
        remainder *= 10;
        rounded.push((remainder / divisor) as u8);
        remainder %= divisor;
    }
    if remainder * 2 >= divisor {
        let mut carry = true;
        for digit in rounded.iter_mut().rev() {
            if *digit == 9 {
                *digit = 0;
            } else {
                *digit += 1;
                carry = false;
                break;
            }
        }
        if carry {
            rounded.insert(0, 1);
        }
    }
    let integer_len = rounded.len() - places;
    let leading = rounded[..integer_len]
        .iter()
        .position(|d| *d != 0)
        .unwrap_or(integer_len - 1);
    let sign = if negative && rounded.iter().any(|d| *d != 0) {
        "-"
    } else {
        ""
    };
    let whole: String = rounded[leading..integer_len]
        .iter()
        .map(|d| char::from(b'0' + d))
        .collect();
    let fractional: String = rounded[integer_len..]
        .iter()
        .map(|d| char::from(b'0' + d))
        .collect();
    Some(format!("{sign}{whole}.{fractional}"))
}
fn rational(n: i128, d: u64) -> Value {
    exact(&ExactFraction {
        numerator: n.to_string(),
        denominator: d,
    })
}
fn exact(f: &ExactFraction) -> Value {
    let native = json!({"numerator":f.numerator,"denominator":f.denominator});
    let mut result = native.clone();
    result["native"] = native;
    result["rounding_mode"] = ROUNDING.into();
    if let Some(values) = [2, 3, 4, 6]
        .into_iter()
        .map(|p| decimal(&f.numerator, f.denominator, p))
        .collect::<Option<Vec<_>>>()
    {
        result["decimal_renderings"] =
            json!({"2":values[0],"3":values[1],"4":values[2],"6":values[3]});
    } else {
        let reason = if f.denominator == 0 {
            "empty_denominator"
        } else {
            "invalid_rational"
        };
        result["status"] = "not_available".into();
        result["reason"] = reason.into();
        result["decimal_renderings"] = unavailable(reason);
    }
    result
}
fn metric_aggregate(m: &MetricStatement) -> Value {
    let (Some(first), Some(last)) = (m.samples.first(), m.samples.last()) else {
        return unavailable("missing_endpoints");
    };
    let (Some(a), Some(b)) = (&first.fraction, &last.fraction) else {
        return unavailable("missing_endpoint_values");
    };
    if a.denominator == 0 || b.denominator == 0 {
        return unavailable("empty_denominator");
    }
    if m.samples.len() < 2
        || m.delta.is_none()
        || m.percentage_point_delta.is_none()
        || a.denominator != b.denominator
    {
        return unavailable("incomparable_components");
    }
    let (Ok(start), Ok(end)) = (a.numerator.parse::<i128>(), b.numerator.parse::<i128>()) else {
        return unavailable("invalid_rational");
    };
    let Some(change) = end.checked_sub(start) else {
        return unavailable("arithmetic_overflow");
    };
    let Some(pp) = change.checked_mul(100) else {
        return unavailable("arithmetic_overflow");
    };
    json!({"population":a.denominator,"start_count":start.to_string(),"end_count":end.to_string(),
        "start_ratio":exact(a),"end_ratio":exact(b),"count_change":change.to_string(),
        "ratio_change":rational(change,a.denominator),"percentage_point_change":rational(pp,a.denominator),
        "source_qualified":m.source_qualified})
}
fn attribution_aggregate(a: &AttributionInput) -> Value {
    if a.denominator == 0 {
        return unavailable("empty_denominator");
    }
    // AttributionInput does not project native source endpoint identities or
    // their interval. Matching population pins, observation/query times or even
    // equal deltas cannot prove that selected metric samples are those endpoints.
    // Retain the independently bound change/sum/residual without inventing a join.
    let endpoints = unavailable("unbound_endpoints");
    let Some(change) = a.delta_bound.as_ref().and_then(|n| n.parse::<i128>().ok()) else {
        return json!({"population":a.denominator,"endpoints":endpoints,
            "ratio_change":unavailable("unbound_aggregate_change"),
            "percentage_point_change":unavailable("unbound_aggregate_change"),
            "component_contribution_sum":contribution_sum(a),
            "reconciliation_residual":unavailable("unbound_aggregate_change"),
            "reconciliation_residual_percentage_points":unavailable("unbound_aggregate_change")});
    };
    let sum = a.contributions.iter().try_fold(0_i128, |n, c| {
        n.checked_add(i128::from(c.newly_absent_count) - i128::from(c.restored_count))
    });
    let (Some(sum), Some(pp)) = (sum, change.checked_mul(100)) else {
        return unavailable("arithmetic_overflow");
    };
    let Some(residual) = change.checked_sub(sum) else {
        return unavailable("arithmetic_overflow");
    };
    let Some(residual_pp) = residual.checked_mul(100) else {
        return unavailable("arithmetic_overflow");
    };
    json!({"population":a.denominator,"endpoints":endpoints,"count_change":change.to_string(),
        "ratio_change":rational(change,a.denominator),"percentage_point_change":rational(pp,a.denominator),
        "component_contribution_sum":rational(sum,a.denominator),
        "reconciliation_residual":rational(residual,a.denominator),
        "reconciliation_residual_percentage_points":rational(residual_pp,a.denominator)})
}
fn contribution_sum(a: &AttributionInput) -> Value {
    a.contributions
        .iter()
        .try_fold(0_i128, |n, c| {
            n.checked_add(i128::from(c.newly_absent_count) - i128::from(c.restored_count))
        })
        .map(|n| rational(n, a.denominator))
        .unwrap_or_else(|| unavailable("arithmetic_overflow"))
}
/// Receives the typed native output and its lossless v1 cell tree. Only explicitly
/// typed rational locations are enriched; Knowledge::Value remains opaque.
pub(super) fn enrich(output: &Output, result: &mut Value) {
    let Output::ProgramReport { report } = output else {
        return;
    };
    for (m, cell) in report.metrics.iter().zip(
        result["report"]["metrics"]
            .as_array_mut()
            .into_iter()
            .flatten(),
    ) {
        cell["aggregate"] = metric_aggregate(m);
        for (s, cell) in m
            .samples
            .iter()
            .zip(cell["samples"].as_array_mut().into_iter().flatten())
        {
            if let Some(f) = &s.fraction {
                cell["fraction"] = exact(f);
            }
        }
        if let Some(f) = &m.delta {
            cell["delta"] = exact(f);
        }
        if let Some(f) = &m.percentage_point_delta {
            cell["percentage_point_delta"] = exact(f);
        }
    }
    for (a, cell) in report.attributions.iter().zip(
        result["report"]["attributions"]
            .as_array_mut()
            .into_iter()
            .flatten(),
    ) {
        // Missing and forbidden references are indistinguishable. Never use a
        // withheld count or inspect the store to explain why a reference is absent.
        if a.attribution.contributions.iter().all(|c| {
            report.basis.iter().any(|v| v.id == c.repository)
                && c.changes
                    .iter()
                    .all(|id| report.basis.iter().any(|v| &v.id == id))
        }) {
            cell["aggregate"] = attribution_aggregate(&a.attribution);
        }
    }
}
