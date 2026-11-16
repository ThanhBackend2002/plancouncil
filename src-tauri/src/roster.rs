//! Danh mục role + phòng ban mặc định (người dùng có thể sửa/xoá/thêm trong Settings).
//!
//! `instructions` viết bằng tiếng Anh (model tuân thủ tốt hơn); output vẫn theo ngôn ngữ của người dùng.

use crate::config::{Appearance, Department, MeetingConfig, PlanDepth, RoleDef, RoleLevel, StaffingMode};

/// 4 role của schema cũ — giữ nguyên id để lịch sử phiên cũ vẫn hiển thị đúng.
pub const LEGACY_ARCHITECT: &str = "architect";
pub const LEGACY_REVIEWERS: [&str; 3] = ["scale_reviewer", "chaos_reviewer", "pragmatic_reviewer"];

/// (gender, hair_style, hair_color, outfit, eyewear, accessory) — chuỗi rỗng = tự sinh.
type Look = (&'static str, &'static str, &'static str, &'static str, &'static str, &'static str);

struct Seed {
    id: &'static str,
    look: Look,
    name: &'static str,
    title: &'static str,
    level: RoleLevel,
    icon: &'static str,
    color: &'static str,
    description: &'static str,
    instructions: &'static str,
}

const SEEDS: &[Seed] = &[
    // ───────────── C-Level ─────────────
    Seed {
        id: "ceo",
        look: ("female", "bob", "black", "blazer", "", "crown"),
        name: "CEO",
        title: "Chief Executive Officer",
        level: RoleLevel::CLevel,
        icon: "crown",
        color: "#f59e0b",
        description: "Tầm nhìn, giá trị kinh doanh, ưu tiên, quyết định go/no-go",
        instructions: "You own the company vision and final business decision. Judge whether the plan creates real business value, \
fits strategy and priorities, has a credible ROI and time-to-market, and whether its scope matches the company's stage. \
Challenge vanity work, unclear success metrics and plans that cannot be explained to investors or customers.",
    },
    Seed {
        id: "cto",
        look: ("male", "side_part", "gray", "suit", "glasses", ""),
        name: "CTO",
        title: "Chief Technology Officer",
        level: RoleLevel::CLevel,
        icon: "cpu",
        color: "#a78bfa",
        description: "Chiến lược công nghệ, định hướng kiến trúc, build vs buy, nợ kỹ thuật",
        instructions: "You own technology strategy. Evaluate architectural direction, long-term scalability, build-vs-buy decisions, \
vendor lock-in, technical debt created, hiring/skill implications and alignment with the existing platform. \
Reject designs that will not survive 10x growth or that the organisation cannot maintain.",
    },
    Seed {
        id: "cpo",
        look: ("female", "long", "brown", "blazer", "", "earrings"),
        name: "CPO",
        title: "Chief Product Officer",
        level: RoleLevel::CLevel,
        icon: "target",
        color: "#22d3ee",
        description: "Chiến lược sản phẩm, giá trị cho người dùng, roadmap, chỉ số thành công",
        instructions: "You own product strategy. Check that the plan solves a real user problem, defines success metrics (activation, \
retention, conversion), has a sensible MVP and roadmap, and does not ship features users did not ask for.",
    },
    Seed {
        id: "cfo",
        look: ("male", "buzz", "black", "suit", "glasses", ""),
        name: "CFO",
        title: "Chief Financial Officer",
        level: RoleLevel::CLevel,
        icon: "wallet",
        color: "#34d399",
        description: "Chi phí, ngân sách, chi phí hạ tầng, unit economics",
        instructions: "You own the budget. Estimate infrastructure, licensing, third-party API and headcount cost with numbers; \
check unit economics (cost per user/request), cost growth with scale, and whether cheaper alternatives exist. \
Reject plans with unbounded or unestimated spend.",
    },
    Seed {
        id: "coo",
        look: ("female", "bun", "dark_brown", "suit", "", ""),
        name: "COO",
        title: "Chief Operating Officer",
        level: RoleLevel::CLevel,
        icon: "settings",
        color: "#fb923c",
        description: "Vận hành, quy trình, nhân sự, tiến độ bàn giao",
        instructions: "You own operations and delivery. Check staffing, ownership, processes, support/on-call model, vendor management, \
operational runbooks and whether the timeline is realistic for the organisation.",
    },
    Seed {
        id: "ciso",
        look: ("male", "short", "black", "suit", "sunglasses", ""),
        name: "CISO",
        title: "Chief Information Security Officer",
        level: RoleLevel::CLevel,
        icon: "shield",
        color: "#f43f5e",
        description: "An ninh tổng thể, threat model, tuân thủ, bảo vệ dữ liệu",
        instructions: "You own security posture. Build a threat model, check authn/authz, secrets, encryption in transit/at rest, \
PII handling, audit logging, compliance (GDPR, PCI, SOC2 where relevant) and incident response readiness.",
    },
    // ───────────── Head / Director ─────────────
    Seed {
        id: "vp_engineering",
        look: ("male", "curly", "black", "blazer", "", ""),
        name: "VP Engineering",
        title: "Vice President of Engineering",
        level: RoleLevel::Head,
        icon: "users",
        color: "#818cf8",
        description: "Năng lực bàn giao, cơ cấu đội, quy trình kỹ thuật",
        instructions: "You own engineering delivery. Check team topology, ownership boundaries, estimates, sequencing of milestones, \
engineering process (code review, CI, release) and whether the plan can be executed by the current teams.",
    },
    Seed {
        id: "head_of_data",
        look: ("female", "ponytail", "auburn", "cardigan", "glasses", ""),
        name: "Head of Data",
        title: "Head of Data & Analytics",
        level: RoleLevel::Head,
        icon: "database",
        color: "#2dd4bf",
        description: "Chiến lược dữ liệu, tracking, data platform, quản trị dữ liệu",
        instructions: "You own data strategy. Check the tracking plan, event schema, data pipelines, warehouse design, data quality, \
retention policies, privacy and how decisions will be measured.",
    },
    // ───────────── Manager ─────────────
    Seed {
        id: "product_manager",
        look: ("female", "wavy", "blonde", "cardigan", "", "badge"),
        name: "Product Manager",
        title: "Product Manager",
        level: RoleLevel::Manager,
        icon: "clipboard",
        color: "#38bdf8",
        description: "Yêu cầu, phạm vi, user story, tiêu chí nghiệm thu, độ ưu tiên",
        instructions: "You own requirements. Turn the request into clear scope, user stories and acceptance criteria; flag ambiguous \
requirements, missing user journeys, scope creep and missing prioritisation (must/should/could).",
    },
    Seed {
        id: "project_manager",
        look: ("male", "short", "brown", "shirt", "", "mug"),
        name: "Project Manager",
        title: "Project Manager",
        level: RoleLevel::Manager,
        icon: "calendar",
        color: "#60a5fa",
        description: "Timeline, milestone, phụ thuộc, quản lý rủi ro",
        instructions: "You own the schedule. Check milestones, critical path, cross-team dependencies, resourcing, risk register and \
communication plan. Flag unrealistic dates and hidden dependencies.",
    },
    Seed {
        id: "business_analyst",
        look: ("female", "bob", "dark_brown", "shirt", "glasses", "clipboard"),
        name: "Business Analyst",
        title: "Business Analyst",
        level: RoleLevel::Manager,
        icon: "search",
        color: "#93c5fd",
        description: "Làm rõ nghiệp vụ, business rule, luồng dữ liệu nghiệp vụ",
        instructions: "You own business rules. Check that every business rule, state transition and calculation is specified, \
including edge cases (refunds, cancellations, time zones, currencies, permissions) and data ownership.",
    },
    Seed {
        id: "ua_manager",
        look: ("female", "twin_tails", "pink", "tshirt", "", "headset"),
        name: "UA Manager",
        title: "User Acquisition Manager",
        level: RoleLevel::Manager,
        icon: "megaphone",
        color: "#f472b6",
        description: "UA: kênh quảng cáo, attribution (MMP), CPI/ROAS, tracking cài đặt",
        instructions: "You own User Acquisition. Check attribution setup (MMP such as AppsFlyer/Adjust, SKAdNetwork/AdAttributionKit, \
Privacy Sandbox), install/event postbacks, deep links, campaign tracking, creative testing, CPI/CPA/ROAS targets, \
cohort reporting and fraud protection. Flag anything that breaks attribution or makes ROAS unmeasurable.",
    },
    Seed {
        id: "monetization_manager",
        look: ("male", "side_part", "blonde", "polo", "", ""),
        name: "Monetization Manager",
        title: "Monetization Manager",
        level: RoleLevel::Manager,
        icon: "coins",
        color: "#facc15",
        description: "Monet: IAP, quảng cáo (mediation), giá, ARPDAU, LTV, A/B test",
        instructions: "You own monetization. Check IAP catalogue and receipt validation, subscriptions, ad mediation (waterfall/bidding), \
ad placements and frequency caps, pricing and localisation, economy balance, ARPDAU/ARPPU/LTV impact, \
and A/B testing of monetization changes without hurting retention.",
    },
    Seed {
        id: "liveops_manager",
        look: ("female", "ponytail", "purple", "hoodie", "", "headphones"),
        name: "LiveOps Manager",
        title: "LiveOps Manager",
        level: RoleLevel::Manager,
        icon: "radio",
        color: "#e879f9",
        description: "Sự kiện, remote config, lịch nội dung, segment người chơi",
        instructions: "You own live operations. Check remote config, feature flags, event scheduling, content pipeline, player \
segmentation, push/in-app messaging, rollback of bad configs and the tooling needed by non-engineers.",
    },
    // ───────────── Lead / Architect ─────────────
    Seed {
        id: LEGACY_ARCHITECT,
        look: ("male", "short", "dark_brown", "polo", "glasses", ""),
        name: "Solution Architect",
        title: "Solution Architect",
        level: RoleLevel::Lead,
        icon: "compass",
        color: "#a78bfa",
        description: "Viết plan kỹ thuật tổng, tổng hợp ý kiến các phòng ban",
        instructions: "You are a principal software architect who writes production-grade technical plans. Be concrete: name \
components, protocols, data stores, schemas, capacity estimates with numbers, SLOs and failure handling.",
    },
    Seed {
        id: "pragmatic_reviewer",
        look: ("female", "bun", "black", "hoodie", "glasses", "mug"),
        name: "Tech Lead",
        title: "Pragmatic Tech Lead",
        level: RoleLevel::Lead,
        icon: "wrench",
        color: "#fbbf24",
        description: "Over-engineering, chi phí vận hành, MVP, observability, kỹ năng đội",
        instructions: "Hunt for: over-engineering and YAGNI, components a small team cannot operate, infrastructure and licensing cost, \
on-call burden, missing observability (logs/metrics/traces), unrealistic scope, build-vs-buy mistakes, missing MVP/milestone \
path, migration and rollback strategy, team skill fit.",
    },
    Seed {
        id: "qa_lead",
        look: ("female", "long", "brown", "polo", "glasses", "badge"),
        name: "QA Lead",
        title: "QA Lead",
        level: RoleLevel::Lead,
        icon: "badge-check",
        color: "#4ade80",
        description: "Chiến lược kiểm thử, quality gate, định nghĩa Done",
        instructions: "You own the test strategy. Check the test pyramid (unit/integration/e2e), test environments and data, quality \
gates in CI, definition of done, regression strategy, performance/load testing and release sign-off criteria.",
    },
    // ───────────── Engineer ─────────────
    Seed {
        id: "scale_reviewer",
        look: ("male", "spiky", "black", "hoodie", "", "headphones"),
        name: "Performance Engineer",
        title: "Scale & Performance Engineer",
        level: RoleLevel::Engineer,
        icon: "gauge",
        color: "#38bdf8",
        description: "Bottleneck, p99 latency, contention, memory, cache, khả năng scale",
        instructions: "Hunt for: throughput ceilings and bottlenecks, p99/p999 latency budget violations, contention (locks, hot keys, \
hot partitions), missing backpressure, unbounded queues/buffers, memory allocation patterns and leaks, N+1 queries, missing \
indexes, sync I/O on hot paths, cache stampede/invalidation, fan-out amplification, horizontal scaling limits. \
Back claims with rough numbers whenever possible.",
    },
    Seed {
        id: "chaos_reviewer",
        look: ("male", "mohawk", "green", "hoodie", "sunglasses", ""),
        name: "Security Engineer",
        title: "Chaos & Security Engineer",
        level: RoleLevel::Engineer,
        icon: "shield-alert",
        color: "#f472b6",
        description: "SPOF, partition, race condition, idempotency, authn/authz, rò rỉ dữ liệu",
        instructions: "Hunt for: single points of failure, behaviour under network partition / split-brain, missing timeouts/retries/\
idempotency keys, race conditions and TOCTOU, ordering and exactly-once fallacies, data corruption and consistency gaps, \
backup/restore and disaster recovery, authn/authz holes, injection (SQL/command/prompt), secrets handling, PII/data leaks, \
multi-tenant isolation, supply-chain risk.",
    },
    Seed {
        id: "backend_dev",
        look: ("male", "curly", "brown", "tshirt", "glasses", "headphones"),
        name: "Backend Developer",
        title: "Senior Backend Developer",
        level: RoleLevel::Engineer,
        icon: "server",
        color: "#60a5fa",
        description: "API, business logic, database, xử lý lỗi, tính khả thi khi code",
        instructions: "You will implement the backend. Check API contracts (versioning, pagination, error format), data model and \
migrations, transactions, validation, error handling, background jobs, and whether each component is actually implementable \
with the chosen stack. Point out missing details you would need before writing code.",
    },
    Seed {
        id: "frontend_dev",
        look: ("female", "bob", "purple", "tshirt", "", "headphones"),
        name: "Frontend Developer",
        title: "Senior Frontend Developer",
        level: RoleLevel::Engineer,
        icon: "monitor",
        color: "#c084fc",
        description: "UI state, hiệu năng client, API contract với FE, offline, i18n",
        instructions: "You will implement the client. Check client state management, loading/error/empty states, API shape for the UI, \
real-time updates, caching, offline behaviour, bundle size, accessibility, i18n and browser/device support.",
    },
    Seed {
        id: "mobile_dev",
        look: ("male", "spiky", "blue", "hoodie", "", ""),
        name: "Mobile Developer",
        title: "Mobile Developer (iOS/Android)",
        level: RoleLevel::Engineer,
        icon: "smartphone",
        color: "#a3e635",
        description: "iOS/Android, app store, offline, push, SDK, kích thước app",
        instructions: "You will implement the mobile apps. Check app store policies, OS permission flows, offline sync and conflict \
resolution, push notifications, SDK integrations (ads, analytics, MMP), app size, battery/network usage, \
forced-update strategy and backward compatibility with old app versions.",
    },
    Seed {
        id: "devops_sre",
        look: ("male", "buzz", "dark_brown", "tshirt", "", "beanie"),
        name: "DevOps / SRE",
        title: "DevOps & Site Reliability Engineer",
        level: RoleLevel::Engineer,
        icon: "cloud",
        color: "#22d3ee",
        description: "CI/CD, hạ tầng (IaC), monitoring, SLO, incident, rollback",
        instructions: "You will run it in production. Check infrastructure as code, environments, CI/CD and deployment strategy \
(blue-green/canary), rollback, autoscaling, SLOs/SLIs and alerting, logging/metrics/tracing, backups, disaster recovery, \
capacity planning and on-call runbooks.",
    },
    Seed {
        id: "dba",
        look: ("male", "bald", "gray", "polo", "glasses", ""),
        name: "Database Engineer",
        title: "Database Engineer / DBA",
        level: RoleLevel::Engineer,
        icon: "database",
        color: "#14b8a6",
        description: "Schema, index, migration, replication, backup, query performance",
        instructions: "You own the databases. Check schema design and normalisation, indexes for every hot query, data growth, \
partitioning/sharding, replication and failover, online migrations without downtime, backup/restore RPO/RTO and \
connection pool sizing.",
    },
    Seed {
        id: "data_engineer",
        look: ("female", "wavy", "black", "hoodie", "", ""),
        name: "Data Engineer",
        title: "Data Engineer",
        level: RoleLevel::Engineer,
        icon: "workflow",
        color: "#5eead4",
        description: "Pipeline, ETL/ELT, streaming, data warehouse, chất lượng dữ liệu",
        instructions: "You build the data pipelines. Check event ingestion, schema evolution, batch vs streaming choice, late/duplicate \
events, idempotent processing, warehouse modelling, data quality checks, backfills and cost of storage/compute.",
    },
    Seed {
        id: "ml_engineer",
        look: ("male", "long", "auburn", "lab_coat", "glasses", ""),
        name: "ML Engineer",
        title: "Machine Learning Engineer",
        level: RoleLevel::Engineer,
        icon: "brain",
        color: "#f0abfc",
        description: "Model, feature, serving, đánh giá, drift, chi phí inference",
        instructions: "You own ML/AI components. Check problem framing, training data and labels, feature pipelines, offline/online \
evaluation metrics, serving latency and cost, fallback when the model fails, drift monitoring and responsible-AI risks.",
    },
    // ───────────── Specialist / Staff ─────────────
    Seed {
        id: "qa_engineer",
        look: ("male", "short", "blonde", "polo", "", "badge"),
        name: "QA Engineer",
        title: "QA Automation Engineer",
        level: RoleLevel::Specialist,
        icon: "flask",
        color: "#86efac",
        description: "Test plan, test case, automation, coverage, test data",
        instructions: "You write and automate the tests. Derive concrete test scenarios from the plan, identify untestable parts, \
missing test hooks/environments, test data needs, contract tests between services and flaky-test risks.",
    },
    Seed {
        id: "qc_specialist",
        look: ("female", "ponytail", "dark_brown", "lab_coat", "glasses", "clipboard"),
        name: "QC Specialist",
        title: "Quality Control Specialist",
        level: RoleLevel::Specialist,
        icon: "list-checks",
        color: "#bef264",
        description: "Kiểm soát chất lượng đầu ra, checklist nghiệm thu, tiêu chuẩn lỗi",
        instructions: "You control output quality before release. Check acceptance checklists, defect severity definitions, \
compliance with the specification and standards, release criteria, traceability from requirement to test, \
and what evidence is required to accept each deliverable.",
    },
    Seed {
        id: "tester",
        look: ("female", "twin_tails", "auburn", "tshirt", "", "clipboard"),
        name: "Tester",
        title: "Manual / Exploratory Tester",
        level: RoleLevel::Specialist,
        icon: "bug",
        color: "#fde047",
        description: "Exploratory test, kịch bản người dùng thật, input bất thường",
        instructions: "You break things like a real user. List concrete scenarios the plan forgets: invalid/huge/unicode inputs, \
double clicks, slow or lost network, app killed mid-flow, clock changes, concurrent sessions on two devices, \
permission denied, upgrade from old versions.",
    },
    Seed {
        id: "ui_ux_designer",
        look: ("female", "wavy", "pink", "dress", "", "flower"),
        name: "UI/UX Designer",
        title: "Product Designer (UI/UX)",
        level: RoleLevel::Specialist,
        icon: "palette",
        color: "#fb7185",
        description: "User flow, usability, accessibility, design system",
        instructions: "You own the user experience. Check user flows end to end, friction points, empty/error/loading states, \
accessibility (contrast, keyboard, screen readers), consistency with the design system and how the UX will be validated.",
    },
    Seed {
        id: "data_analyst",
        look: ("male", "side_part", "black", "cardigan", "glasses", ""),
        name: "Data Analyst",
        title: "Data Analyst",
        level: RoleLevel::Specialist,
        icon: "chart",
        color: "#67e8f9",
        description: "KPI, tracking plan, dashboard, phân tích A/B test",
        instructions: "You measure the outcome. Check KPIs and their exact definitions, the tracking plan (events + properties), \
dashboards, A/B test design (hypothesis, sample size, guardrail metrics) and whether success can be proven.",
    },
    Seed {
        id: "legal_compliance",
        look: ("female", "bun", "gray", "suit", "glasses", ""),
        name: "Legal & Compliance",
        title: "Legal & Compliance Officer",
        level: RoleLevel::Specialist,
        icon: "scale",
        color: "#cbd5e1",
        description: "Pháp lý, quyền riêng tư (GDPR), giấy phép, chính sách store",
        instructions: "You own legal risk. Check privacy (GDPR/CCPA/PDPA, consent, data residency), terms of service, open-source \
licences, app store and ad network policies, age ratings/COPPA, financial regulations and contractual obligations.",
    },
];

pub fn default_roles(provider_id: &str, model: &str, temperature: f32) -> Vec<RoleDef> {
    SEEDS
        .iter()
        .map(|s| RoleDef {
            id: s.id.into(),
            name: s.name.into(),
            title: s.title.into(),
            level: s.level,
            icon: s.icon.into(),
            color: s.color.into(),
            description: s.description.into(),
            instructions: s.instructions.into(),
            provider_id: provider_id.into(),
            model: model.into(),
            temperature,
            appearance: {
                let (g, hs, hc, o, e, a) = s.look;
                Appearance::new(g, hs, hc, o, e, a)
            },
        })
        .collect()
}

/// Ngoại hình mẫu của role trong danh mục (để điền cho config tạo trước khi có `appearance`).
pub fn seed_appearance(id: &str) -> Option<Appearance> {
    SEEDS.iter().find(|s| s.id == id).map(|s| {
        let (g, hs, hc, o, e, a) = s.look;
        Appearance::new(g, hs, hc, o, e, a)
    })
}

fn dept(id: &str, name: &str, mission: &str, lead: &str, members: &[&str]) -> Department {
    Department {
        id: id.into(),
        name: name.into(),
        mission: mission.into(),
        lead_id: lead.into(),
        member_ids: members.iter().map(|m| (*m).to_owned()).collect(),
        max_rounds: 2,
    }
}

pub fn default_departments() -> Vec<Department> {
    vec![
        dept(
            "board",
            "Ban Giám đốc",
            "Business value, budget, priorities, operating model and go/no-go recommendation.",
            "ceo",
            &["cto", "cfo", "coo", "cpo"],
        ),
        dept(
            "product",
            "Product & Design",
            "Requirements, user stories, acceptance criteria, user flows and success metrics.",
            "product_manager",
            &["business_analyst", "ui_ux_designer", "data_analyst"],
        ),
        dept(
            "engineering",
            "Engineering",
            "Implementation plan: services, APIs, data model, client apps, milestones and estimates.",
            "pragmatic_reviewer",
            &["backend_dev", "frontend_dev", "mobile_dev", "dba"],
        ),
        dept(
            "platform",
            "Platform & SRE",
            "Infrastructure, deployment, scalability, observability, reliability and disaster recovery.",
            "devops_sre",
            &["scale_reviewer", "dba"],
        ),
        dept(
            "qa",
            "QA & QC",
            "Test strategy, test scenarios, automation, quality gates and release acceptance.",
            "qa_lead",
            &["qa_engineer", "qc_specialist", "tester"],
        ),
        dept(
            "security",
            "Security & Compliance",
            "Threat model, security controls, privacy, legal and policy compliance.",
            "ciso",
            &["chaos_reviewer", "legal_compliance"],
        ),
        dept(
            "growth",
            "Growth (UA & Monetization)",
            "User acquisition, attribution, monetization (IAP/ads), LiveOps and growth analytics.",
            "ua_manager",
            &["monetization_manager", "liveops_manager", "data_analyst"],
        ),
        dept(
            "data",
            "Data & AI",
            "Tracking plan, data pipelines, warehouse, ML components and data governance.",
            "head_of_data",
            &["data_engineer", "ml_engineer", "data_analyst"],
        ),
    ]
}

/// Cuộc họp mặc định giữ đúng hành vi cũ: Architect chủ trì, 3 reviewer, không có phòng ban.
pub fn default_meeting() -> MeetingConfig {
    MeetingConfig {
        chair_id: LEGACY_ARCHITECT.into(),
        reviewer_ids: LEGACY_REVIEWERS.iter().map(|s| (*s).to_owned()).collect(),
        department_ids: vec![],
        // Ép REJECT ở vòng 1 khiến reviewer phải "tìm ra" vấn đề → plan phình to. Mặc định tắt.
        strict_first_round: false,
        plan_depth: PlanDepth::Standard,
        staffing: StaffingMode::Suggest,
        required_role_ids: vec![],
        max_auto_reviewers: 5,
        max_auto_departments: 3,
        dynamic_membership: true,
        max_invites: 2,
        section_routing: true,
        max_total_tokens: None,
        revise_mode: crate::config::ReviseMode::Full,
        section_mode_max_ratio: 0.5,
        consistency_checker: crate::config::CONSISTENCY_CHECKER_AUTHOR.into(),
    }
}
