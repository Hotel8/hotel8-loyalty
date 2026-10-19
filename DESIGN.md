# Hotel8:Loyalty - Detailed Design Document

## 1. Scope & Context

**Owned by Loyalty:** database `hotel8`, schema `loyalty` - 8 tables
(`loyalty.programs`, `members`, `point_transactions`, `tier_rules`,
`redemption_catalog`, `redemptions` + `loyalty.tier` / `loyalty.txn_type`
enums + `loyalty.outbox` / `loyalty.audit_journal`); event family `loyalty`;
the stay-points direct consumer; RFW admin pages.

**NOT owned:** guest identity (Sentin8:Auth), guest profiles/stays
(hotel8:pms; the member-link is PMS's field), reward usage at tables
(hotel8:pos calls redemption), notification delivery (Sentin8:Post).

### System context

```mermaid
graph TD
    subgraph "hotel8 vertical"
        L[hotel8:loyalty]
        P3[hotel8:pms] -- "hotel8:pms:reservation:checked_out (consumer loyalty-awards)" --> L
        POS2[hotel8:pos] -. "reward redemption (HTTP)" .-> L
    end
    subgraph "platform"
        INGL[Sentin8:Proxy] -- "context headers" --> L
        BUSL[Sentin8:Bus] <-- "produce (M2M)" --> RLL[shell relay]
        RLL --- L
    end
    DBL[(db hotel8 / schema loyalty)] --> L
```

### Container view

```mermaid
graph TD
    subgraph loyaltys["hotel8:loyalty"]
        H5[/programs members points rewards admin/]
        TIER[tier progression]
        LEDGER[point ledger]
        RED[redemption service]
        OUTL[(loyalty.outbox)]
        AJL[(loyalty.audit_journal)]
    end
    H5 --> TIER --> DBL2[(loyalty.* tables)]
    LEDGER --> OUTL
```

## 2. Runtime Architecture

```mermaid
graph LR
    BOOTL[shell::serve] --> ENGL[Shield]
    BOOTL --> RPL[(pool cap 3)]
    BOOTL --> RLL2[relay]
    BOOTL --> PSHL[audit pusher]
    CONL[consumer: checked_out] -- SSE --> BUSL2[("stream loyalty intake")]
```

### Request lifecycle - stay points (event path)

```mermaid
sequenceDiagram
    participant BUS as Sentin8:Bus (stream loyalty)
    participant L as hotel8:loyalty consumer
    participant DB as db hotel8 (loyalty)
    BUS->>L: hotel8:pms:reservation:checked_out
    L->>DB: set_tenant_context(payload.env_id)
    L->>DB: resolve member via profile link (event payload refs)
    L->>DB: compute earn (tier rule × folio spend snapshot)
    alt member + program active
        L->>DB: INSERT loyalty.point_transactions (earn)
        L->>DB: INSERT loyalty.outbox (hotel8:loyalty:points:earned)
        L->>BUS: ack
    else rule declines
        L->>BUS: ack (no-op)
    end
```

### Request lifecycle - redemption (API path)

```mermaid
sequenceDiagram
    participant C as Client (BFF)
    participant L as hotel8:loyalty
    participant S as Shield
    participant DB as db hotel8
    C->>L: POST /v1/redemptions {member, reward}
    L->>S: require(hotel8:loyalty:points:write)
    S-->>L: permit
    L->>DB: BEGIN, balance check, INSERT redemption + txn (redeem), outbox, journal, COMMIT
    alt insufficient balance
        L-->>C: 422 with remaining balance
    end
    L-->>C: 200 redemption
```

## 3. Data Model

```mermaid
erDiagram
    loyalty_programs ||--o{ loyalty_tier_rules : defines
    loyalty_programs ||--o{ loyalty_members : enrolls
    loyalty_members ||--o{ loyalty_point_transactions : ledger
    loyalty_programs ||--o{ loyalty_redemption_catalog : offers
    loyalty_members ||--o{ loyalty_redemptions : redeems
    loyalty_redemption_catalog ||--o{ loyalty_redemptions : fulfills
```

- `tier`: BASE/SILVER/GOLD/PLATINUM/DIAMOND enum · `txn_type`:
  EARN/REDEEM/ADJUST/EXPIRE/TRANSFER enum.
- All tables env-scoped + FORCE-RLS; audit journal included.

### Tier progression

```mermaid
stateDiagram-v2
    [*] --> BASE: enroll
    BASE --> SILVER: rules threshold
    SILVER --> GOLD: rules threshold
    GOLD --> PLATINUM: rules threshold
    PLATINUM --> DIAMOND: rules threshold
```

## 4. API Surface

**Public** (member realm via BFF; `hotel8:loyalty:{read}`): member profile +
balance, transaction history, catalog browse.

**Member mutations** (`hotel8:loyalty:points:write`): redeem reward,
transfer (where program permits).

**Admin** (`hotel8:loyalty:admin` + `hotel8:loyalty:points:write`): programs
CRUD, tier rules, points adjust, member link (`member:linked`),
redemption catalog CRUD, EOD-tier review (`tiers:reviewed`).

### Cross-service data access - decision order (membus → bus → owner API)

Awards accrual is event-first: `hotel8:pms:reservation:checked_out`
(consumer group `loyalty-awards`) triggers earning - the bus path. The
residual synchronous path is the PMS owner API for profile
write-back when a member links (`set_loyalty_number`) - a read-your-writes
mutation that must return the updated profile to the caller in the same
request. MemBus: none (member balances change at transaction pace, not tick
pace; newest-wins adds nothing). No pms-schema grant - loyalty reads PMS
only through its API, and never the reverse (PMS holds no loyalty grants).

### Event flow

```mermaid
sequenceDiagram
    participant DB as loyalty.outbox
    participant R as shell relay
    participant B as Sentin8:Bus
    DB->>R: NOTIFY + poll
    R->>B: M2M produce
    alt ok
        B-->>R: 200 → relayed
    else down
        DB->>DB: attempts++ → DLQ at max
    end
```

## 5. Cross-Cutting Concerns

Auth/tenant/RLS/audit/drain/TLS pool - the platform contract, shell-owned;
tier-aware ABAC (points:adjust authority) is tenant-authored.

## 6. Failure Modes & Recovery

```mermaid
flowchart TD
    A[checked_out replay] --> B[idempotent per reservation - no double award]
    C[redemption race] --> D{balance >= cost in tx?}
    D -->|no| E[422 - remaining balance returned]
    D -->|yes| F[commit]
    G[Bus outage] --> H[outbox persists; relay retries]
```

## 7. Concurrency & Performance Model

```mermaid
graph LR
    REQS --> MWL[middleware]
    MWL --> HL[handlers]
    HL --> CHL[("pool cap 3")]
    RLYL[relay] --> BL[("bus stream loyalty")]
```

- Balance check is in-transaction (SELECT then conditional INSERT of the
  redeem txn) - no SELECT-then-act race.
- Event payload carries the spend snapshot - no PMS read-back on the hot
  path.

## 8. Invariants, Insecure-Defaults Rejected, Unsafe Rationale

- Balance = Σ ledger by movement type; adjustments self-document their
  actor.
- A redemption never sends the balance negative.
- Award idempotency per reservation (replay-safe).
- No `unsafe`; no platform-DB access; no secrets.
- Insecure defaults rejected: no direct PMS folio reads (spend arrives in
  event payload); no self-service tier promotion.

## 9. Testing Strategy

```mermaid
graph TD
    UL[Unit - tier + earn rules, ledger math] --> IL[Integration - consumer idempotency, policy parity]
    IL --> EL[E2E - member + admin surfaces via BFF]
```

## 10. Open Questions

- Airline/partner transfer support.
- Benefit catalogs beyond points (gift/match tiers).

---

**Design attribution:** hotel8:loyalty and its underlying concepts were designed by Hamid Jahanzad <Hamid@Jahanzad.com>.
