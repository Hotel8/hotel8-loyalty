// SPDX-FileCopyrightText: Copyright Logically Magic Inc. <hello@logicallymagic.com>
// SPDX-FileCopyrightText: Copyright Hamid Jahanzad <Hamid@Jahanzad.com>
// SPDX-FileCopyrightText: Copyright Contributors to the Hotel8:Loyalty
// SPDX-License-Identifier: Apache-2.0

//! Pure loyalty rules - DB-free, testable.
//!
//! Tier progression over the `hotel8.loyalty_tier` DB enum labels
//! (BASE/SILVER/GOLD/PLATINUM/DIAMOND - see `services/loyalty/sql/init.sql`)
//! and the point math (accrual truncation, redemption guard, adjust
//! clamping, expiry zero-out). The handlers call these fns; the SQL remains
//! the source of truth for stored values.

/// Loyalty tiers, ordered by progression.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tier {
    Base,
    Silver,
    Gold,
    Platinum,
    Diamond,
}

impl Tier {
    /// DB enum label (`hotel8.loyalty_tier` value).
    pub fn label(self) -> &'static str {
        match self {
            Tier::Base => "BASE",
            Tier::Silver => "SILVER",
            Tier::Gold => "GOLD",
            Tier::Platinum => "PLATINUM",
            Tier::Diamond => "DIAMOND",
        }
    }

    /// Parse a DB label (unknown → BASE).
    pub fn from_label(s: &str) -> Self {
        match s {
            "SILVER" => Tier::Silver,
            "GOLD" => Tier::Gold,
            "PLATINUM" => Tier::Platinum,
            "DIAMOND" => Tier::Diamond,
            _ => Tier::Base,
        }
    }

    /// Minimum points balance that reaches this tier.
    pub fn min_points(self) -> i64 {
        match self {
            Tier::Base => 0,
            Tier::Silver => 5_000,
            Tier::Gold => 15_000,
            Tier::Platinum => 40_000,
            Tier::Diamond => 100_000,
        }
    }
}

/// The tier a member belongs to at a given points balance.
pub fn tier_for_balance(balance: i64) -> Tier {
    let mut best = Tier::Base;
    for t in [Tier::Silver, Tier::Gold, Tier::Platinum, Tier::Diamond] {
        if balance >= t.min_points() {
            best = t;
        }
    }
    best
}

/// Count the tier upgrades implied by a balance change (0 or more).
pub fn upgrades(old_balance: i64, new_balance: i64) -> usize {
    let (o, n) = (tier_for_balance(old_balance), tier_for_balance(new_balance));
    if n > o {
        n as usize - o as usize
    } else {
        0
    }
}

/// Count the tier downgrades implied by a balance change (0 or more).
pub fn downgrades(old_balance: i64, new_balance: i64) -> usize {
    let (o, n) = (tier_for_balance(old_balance), tier_for_balance(new_balance));
    if n < o {
        o as usize - n as usize
    } else {
        0
    }
}

/// Points earned for a stay: spend × points-per-currency × multiplier,
/// truncated to whole points (partial points are never credited).
pub fn points_earned(spend: f64, points_per_currency: f64, multiplier: f64) -> i64 {
    ((spend * points_per_currency * multiplier) as i64).max(0)
}

/// Whether a redemption is allowed (must cover the full cost).
pub fn can_redeem(balance: i64, cost: i64) -> bool {
    cost > 0 && balance >= cost
}

/// Apply a signed adjustment clamped at zero (balance never goes negative).
pub fn adjust(balance: i64, delta: i64) -> i64 {
    (balance + delta).max(0)
}

/// Expiry zero-out: expired points reduce the balance to zero floor.
pub fn expire(balance: i64, expired: i64) -> i64 {
    (balance - expired).max(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_match_db_enums() {
        assert_eq!(Tier::Base.label(), "BASE");
        assert_eq!(Tier::Silver.label(), "SILVER");
        assert_eq!(Tier::Gold.label(), "GOLD");
        assert_eq!(Tier::Platinum.label(), "PLATINUM");
        assert_eq!(Tier::Diamond.label(), "DIAMOND");
    }

    #[test]
    fn from_label_round_trips_every_variant() {
        for t in [
            Tier::Base,
            Tier::Silver,
            Tier::Gold,
            Tier::Platinum,
            Tier::Diamond,
        ] {
            assert_eq!(Tier::from_label(t.label()), t);
        }
        assert_eq!(Tier::from_label("garbage"), Tier::Base);
    }

    #[test]
    fn tier_progression_boundaries() {
        assert_eq!(tier_for_balance(0), Tier::Base);
        assert_eq!(tier_for_balance(4_999), Tier::Base);
        assert_eq!(tier_for_balance(5_000), Tier::Silver);
        assert_eq!(tier_for_balance(15_000), Tier::Gold);
        assert_eq!(tier_for_balance(40_000), Tier::Platinum);
        assert_eq!(tier_for_balance(100_000), Tier::Diamond);
        assert_eq!(tier_for_balance(i64::MAX), Tier::Diamond);
    }

    #[test]
    fn upgrade_and_downgrade_counts() {
        assert_eq!(upgrades(0, 5_000), 1);
        assert_eq!(upgrades(0, 100_000), 4);
        assert_eq!(upgrades(10_000, 11_000), 0);
        assert_eq!(downgrades(100_000, 99_999), 1);
        assert_eq!(downgrades(0, 5_000), 0);
    }

    #[test]
    fn points_earned_truncates() {
        assert_eq!(points_earned(100.0, 1.0, 1.0), 100);
        assert_eq!(points_earned(10.5, 1.5, 2.0), 31); // 31.5 → 31
        assert_eq!(points_earned(-5.0, 1.0, 1.0), 0);
    }

    #[test]
    fn redemption_requires_full_cost() {
        assert!(can_redeem(1_000, 500));
        assert!(!can_redeem(500, 1_000));
        assert!(!can_redeem(0, 0)); // zero-cost redemption is not a redemption
        assert!(!can_redeem(1_000, -5));
    }

    #[test]
    fn adjust_clamps_at_zero() {
        assert_eq!(adjust(100, 50), 150);
        assert_eq!(adjust(100, -150), 0);
        assert_eq!(adjust(0, -5), 0);
    }

    #[test]
    fn expire_zeroes_out_floor() {
        assert_eq!(expire(1_000, 400), 600);
        assert_eq!(expire(100, 500), 0);
    }
}
