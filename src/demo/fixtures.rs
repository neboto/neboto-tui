//! The demo dataset: one made-up account, `acme-prod`, with problems planted
//! in it to find. Responses are real wire-format bodies (XML for EC2 / STS,
//! awsJson for ECS) with `{{…}}` placeholders — see `demo::render`.
//!
//! The story so far (PoC — EC2 + ECS):
//! - `web-sg` allows SSH from 0.0.0.0/0 ("temp debug access", never removed)
//! - an unattached 500 GiB gp2 volume left behind by a terminated instance
//! - ECS service `orders-worker`: deployment of revision 15 failed and the
//!   circuit breaker rolled it back — the stopped task says why (exit 1,
//!   missing `DATABASE_URL`)
//!
//! Order matters: the first entry whose `when` substrings all appear in the
//! request body wins, so put narrow matches before the catch-all. (Batch
//! calls like `DescribeTasks` get one fixture per id set the app asks for; a
//! fixture that filters its items by the requested ids is the scalable
//! version, left for when the dataset grows.)

pub struct Fixture {
    pub service: &'static str,
    pub operation: &'static str,
    /// Substrings the request body must all contain (empty = any request).
    pub when: &'static [&'static str],
    pub body: &'static str,
}

macro_rules! fx {
    ($svc:literal, $op:literal, $file:literal) => {
        Fixture {
            service: $svc,
            operation: $op,
            when: &[],
            body: include_str!(concat!("fixtures/", $svc, "/", $file)),
        }
    };
    ($svc:literal, $op:literal, when [$($w:literal),+], $file:literal) => {
        Fixture {
            service: $svc,
            operation: $op,
            when: &[$($w),+],
            body: include_str!(concat!("fixtures/", $svc, "/", $file)),
        }
    };
}

pub static FIXTURES: &[Fixture] = &[
    // ── STS ──────────────────────────────────────────────────────────────────
    fx!("sts", "GetCallerIdentity", "GetCallerIdentity.xml"),
    // ── IAM ──────────────────────────────────────────────────────────────────
    fx!("iam", "ListAccountAliases", "ListAccountAliases.xml"),
    // ── EC2 ──────────────────────────────────────────────────────────────────
    fx!("ec2", "DescribeInstances", "DescribeInstances.xml"),
    fx!("ec2", "DescribeSecurityGroups", "DescribeSecurityGroups.xml"),
    fx!("ec2", "DescribeVolumes", "DescribeVolumes.xml"),
    // ── ECS ──────────────────────────────────────────────────────────────────
    fx!("ecs", "ListClusters", "ListClusters.json"),
    fx!("ecs", "DescribeClusters", "DescribeClusters.json"),
    fx!("ecs", "ListServices", "ListServices.json"),
    fx!("ecs", "DescribeServices", "DescribeServices.json"),
    fx!("ecs", "ListTaskDefinitions", "ListTaskDefinitions.json"),
    fx!("ecs", "DescribeTaskDefinition", when ["orders-worker:15"], "DescribeTaskDefinition-worker-15.json"),
    fx!("ecs", "DescribeTaskDefinition", when ["orders-worker"], "DescribeTaskDefinition-worker-14.json"),
    fx!("ecs", "DescribeTaskDefinition", "DescribeTaskDefinition-web.json"),
    // A service's own tasks (its Tasks section) vs the cluster-wide load.
    // Stopped tasks exist only for orders-worker (the failed rollout).
    fx!("ecs", "ListTasks", when ["STOPPED", "storefront-web"], "ListTasks-none.json"),
    fx!("ecs", "ListTasks", when ["STOPPED"], "ListTasks-stopped.json"),
    fx!("ecs", "ListTasks", when ["orders-worker"], "ListTasks-worker.json"),
    fx!("ecs", "ListTasks", when ["storefront-web"], "ListTasks-web.json"),
    fx!("ecs", "ListTasks", "ListTasks-running.json"),
    // DescribeTasks answers for exactly the ids asked: all running (the
    // cluster-wide load), the stopped pair, or one service's tasks.
    fx!("ecs", "DescribeTasks", when ["0f1e2d3c", "3c4d5e6f"], "DescribeTasks-running.json"),
    fx!("ecs", "DescribeTasks", when ["5d1e0c7a"], "DescribeTasks-stopped.json"),
    fx!("ecs", "DescribeTasks", when ["3c4d5e6f"], "DescribeTasks-worker.json"),
    fx!("ecs", "DescribeTasks", "DescribeTasks-web.json"),
];

/// The body of the first fixture matching `service` / `operation` whose
/// `when` (if any) appears in the request body.
pub fn find(service: &str, operation: &str, request_body: &str) -> Option<&'static str> {
    FIXTURES
        .iter()
        .find(|f| {
            f.service == service
                && f.operation == operation
                && f.when.iter().all(|w| request_body.contains(w))
        })
        .map(|f| f.body)
}
