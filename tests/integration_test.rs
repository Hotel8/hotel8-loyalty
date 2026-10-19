// SPDX-FileCopyrightText: Copyright Logically Magic Inc. <hello@logicallymagic.com>
// SPDX-FileCopyrightText: Copyright Hamid Jahanzad <Hamid@Jahanzad.com>
// SPDX-FileCopyrightText: Copyright Contributors to the Hotel8:Loyalty
// SPDX-License-Identifier: Apache-2.0

//! Hotel8:Loyalty integration tests (test-strategy Layer 3) - DB-free.
//!
//! Exercises the extracted pure rules (`loyalty_rules`) end-to-end the way
//! the handlers use them: tier resolution → award → redeem → adjust → expire.

use hotel8_loyalty_server::loyalty_rules as rules;

#[test]
fn full_tier_lifecycle_member_journey() {
    // New member: BASE at 0 points.
    let mut balance = 0i64;
    let mut tier = rules::tier_for_balance(balance);
    assert_eq!(tier, rules::Tier::Base);

    // A stay earns points (spend 250 × 10 ppc × 1.0 multiplier, truncated).
    let earned = rules::points_earned(250.0, 10.0, 1.0);
    assert_eq!(earned, 2_500);
    balance = rules::adjust(balance, earned);
    assert_eq!(balance, 2_500);
    assert_eq!(rules::tier_for_balance(balance), rules::Tier::Base);
    assert_eq!(rules::upgrades(0, balance), 0);

    // Second, bigger stay crosses into SILVER.
    let earned2 = rules::points_earned(300.0, 10.0, 1.0);
    assert_eq!(earned2, 3_000);
    balance = rules::adjust(balance, earned2);
    assert_eq!(balance, 5_500);
    assert_eq!(rules::tier_for_balance(balance), rules::Tier::Silver);
    assert_eq!(rules::upgrades(0, balance), 1);
    tier = rules::tier_for_balance(balance);
    assert_eq!(tier, rules::Tier::Silver);

    // Redemption: full-cost rule (member tries 6_000 with 5_500).
    assert!(!rules::can_redeem(balance, 6_000));
    balance = rules::adjust(balance, -5_000);
    assert_eq!(balance, 500);
    // Downgrade below SILVER threshold.
    assert_eq!(rules::downgrades(5_000, 500), 1);
    assert_eq!(rules::tier_for_balance(500), rules::Tier::Base);
}

#[test]
fn adjust_never_negative_in_a_series() {
    let mut balance = 0i64;
    for delta in [100, -150, 200, -300, 50] {
        balance = rules::adjust(balance, delta);
        assert!(balance >= 0, "balance went negative: {balance}");
    }
    assert_eq!(balance, 50);
}

#[test]
fn expiry_of_all_points_zeros_the_floor() {
    let balance = rules::expire(2_500, 2_500);
    assert_eq!(balance, 0);
    assert_eq!(rules::tier_for_balance(balance), rules::Tier::Base);
    assert_eq!(rules::downgrades(rules::Tier::Silver.min_points(), 0), 1);
}

#[test]
fn labels_round_trip_every_tier() {
    for t in [
        rules::Tier::Base,
        rules::Tier::Silver,
        rules::Tier::Gold,
        rules::Tier::Platinum,
        rules::Tier::Diamond,
    ] {
        assert_eq!(rules::Tier::from_label(t.label()), t);
    }
}

#[test]
fn multi_tier_upgrade_counts() {
    assert_eq!(rules::upgrades(0, rules::Tier::Diamond.min_points() + 1), 4);
    assert_eq!(
        rules::upgrades(
            rules::Tier::Gold.min_points(),
            rules::Tier::Diamond.min_points()
        ),
        2
    );
    assert_eq!(rules::upgrades(500, 600), 0);
}

#[test]
fn points_earned_rejects_negative_spend() {
    assert_eq!(rules::points_earned(-100.0, 10.0, 1.0), 0);
    assert_eq!(rules::points_earned(0.0, 0.0, 0.0), 0);
}

#[test]
fn redemption_requires_positive_cost() {
    assert!(!rules::can_redeem(10_000, 0));
    assert!(!rules::can_redeem(10_000, -1));
    assert!(rules::can_redeem(10_000, 10_000));
}

#[test]
fn partial_points_truncate_not_round() {
    // 10.5 × 1.5 × 2.0 = 31.5 → 31 (truncation, not 32).
    assert_eq!(rules::points_earned(10.5, 1.5, 2.0), 31);
    assert_eq!(rules::points_earned(10.9, 1.0, 1.0), 10);
}
