# Hotel8:Loyalty - Business Requirements & Functionality

> Audience: product owners, business analysts, and anyone evaluating what
> Hotel8:Loyalty **does for the business**. For architecture read
> `README.md`; for wire-level detail read `DESIGN.md`.

## 1. Business Purpose

- Grow repeat business with a programmable rewards program: earn on stays,
  redeem across hotel and restaurant touchpoints.
- Make program economics (tiers, rates, catalog) tenant-authored data.
- Reward stays automatically the moment the stay completes.

## 2. Business Scope (What the Service Provides)

| Capability | Description |
|---|---|
| Programs | Multiple reward programs per tenant with authored economics |
| Enrollment | Member enrollment with loyalty numbering |
| Member linkage | Member ↔ PMS guest-profile linking (via PMS) |
| Stay points | Automatic earning from PMS check-out by tier rules |
| Ledger | Append-only point transactions (earn/redeem/adjust/expire/transfer) |
| Tiers | Threshold-based progression with per-tier earn multipliers |
| Redemption catalog | Admin-curatable rewards with point costs |
| Redemptions | Redeem rewards (catalog and POS table-side) |
| Expiry | Program-authored expiry windows |
| Admin console | RFW pages incl. shared Shield admin |

**What the service does NOT own (business boundary)**

| Concern | Owning service |
|---|---|
| Guest identity | Sentin8:Auth |
| Guest profiles & stays | hotel8:pms |
| Reward usage at the table | hotel8:pos |
| Offer/upsell economics | hotel8:upsell |
| Notification delivery | Sentin8:Post |
| Money movement of any kind | Bill8:Payments / Bill8:Ledger |

## 3. Business Capabilities

| Area | Business rule |
|---|---|
| Enrollment | A member belongs to a program and carries a unique loyalty number; identity linkage to an auth user is recorded when the member signs in. |
| Earning | Stay points accrue by tier rule × folio spend; awarded exactly once per checked-out stay. |
| Tiering | Tiers progress by program-authored thresholds over earned value; multipliers increase with tier. |
| Redemption | A reward redeems only with sufficient balance; redemption is a ledger movement, never a balance edit. |
| Adjustments | Service recovery adjustments carry the adjusting identity and reason. |
| Expiry | Points expire by program window; expiry is a ledger movement, preserving history. |
| Transfer | Programs may permit member-to-member transfers under policy. |
| Catalog | Admin-curated reward items with costs and availability. |

## 4. Business Events (Outbound)

| Event | Fires when |
|---|---|
| `hotel8:loyalty:program:created`, `tier:configured` | Program configuration |
| `hotel8:loyalty:member:created / linked` | Enrollment and profile linkage |
| `hotel8:loyalty:points:earned / awarded / adjusted / redeemed / expired` | Ledger movements |
| `hotel8:loyalty:reward:created / updated / deleted / redeemed` | Redemption catalog and usage |
| `hotel8:loyalty:tiers:reviewed` | Tier review pass |

**Inbound (consumed):** `hotel8:pms:reservation:checked_out` - awards stay
points.

## 5. Business Rules & Invariants

1. A member's balance is the sum of their ledger; it can never go negative.
2. Points earned from a stay are awarded exactly once per stay.
3. Tier progression follows program thresholds automatically.
4. Every ledger movement records its actor and business reason.
5. Redemption at POS deducts loyalty first-class - the guest never
   double-pays.
6. Program rules changes affect future movements only.

## 6. Target-User Roles & Use Cases

| Persona | Use |
|---|---|
| Guest/member | View balance & history; redeem rewards |
| Front desk / service recovery | Adjust points with reason |
| Program manager | Programs, tiers, catalog authoring |
| Marketing | Campaign-linked earn rules |
| Hotel admin | Policy admin for the surface |

## 7. KPIs / Success Criteria

- Member enrollment vs completed stays ratio.
- Redemption rate and catalog health (no out-of-stock surprises).
- Zero negative balances; zero duplicate awards.
- Stay points awarded without agent action (auto-adjudication rate).

## 8. Dependencies & Reliance on Other Services

| Reliance | For |
|---|---|
| hotel8:pms | Stays, folio snapshots (event payload), profile linkage |
| hotel8:pos | Table-side redemption initiation |
| Sentin8:Auth + Shield | Identity; tier-aware adjust authority (ABAC) |
| Sentin8:Bus | `loyalty` stream + PMS intake |
| Sentin8:Sys | Audit ingestion |
| Sentin8:Post | Member communications |

## 9. Acceptance Criteria

1. A checked-out stay with a linked member auto-awards points per their
   tier.
2. A redemption cannot overdraw a balance.
3. A program's tier rules can be authored with no code change.
4. A redemption performed at the restaurant reduces the member balance once.
5. Point history is complete and auditable from the ledger alone.

## 10. Open Business Questions

- Partner transfers (airlines/retail).
- Benefit catalogs beyond point economics.

---

**Architecture & Design:** hotel8:loyalty and its underlying concepts were designed by [Hamid Jahanzad](https://www.linkedin.com/in/hamidjahanzad/).
