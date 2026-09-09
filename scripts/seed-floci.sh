#!/usr/bin/env bash
#
# Seed a local floci (or LocalStack) AWS emulator with bulk data so you can
# exercise neboto's paging / scrolling / search against realistic volume.
#
# Usage:
#   floci start                         # or: docker compose up -d
#   ./scripts/seed-floci.sh             # defaults: 25 of each, http://localhost:4566
#   COUNT=200 ./scripts/seed-floci.sh   # crank up the volume
#   ENDPOINT=http://localhost:4566 SEED_EKS=1 SEED_LAMBDA=1 ./scripts/seed-floci.sh
#
# Then point neboto at the same endpoint:
#   AWS_ENDPOINT_URL=http://localhost:4566 cargo run
#   (or set `endpoint_url = "http://localhost:4566"` in ~/.neboto.toml)
#
# ── PERSISTENCE (important) ───────────────────────────────────────────────────
# floci defaults to in-memory storage (FLOCI_STORAGE_MODE=memory): seeded data is
# LOST on every restart. To keep it, start floci in a disk-backed mode BEFORE
# seeding:
#   export FLOCI_STORAGE_MODE=persistent           # flush on every write (safest)
#   export FLOCI_STORAGE_PERSISTENT_PATH="$HOME/.floci-data"
#   floci start                                     # then run this seeder once
# Modes: memory (default, RAM only) · persistent (flush per write) ·
#        hybrid (async flush ~5s) · wal (write-ahead log, max durability).
# Via docker-compose: set the same env vars AND mount the data dir as a volume
# (e.g. ./floci-data:/data) so it survives container recreation, not just restart.
# After switching modes the old in-memory data is already gone — re-run this once.
#
# ── COST WARNING ──────────────────────────────────────────────────────────────
# SEED_EKS=1 / SEED_LAMBDA=1 spin up REAL Docker/k8s containers per resource and
# can pin your machine. Leave them off unless you specifically need them; the
# other services are in-process and cheap.
#
# Requires the `aws` CLI. Credentials are dummy — the emulator ignores them.

set -uo pipefail

ENDPOINT="${ENDPOINT:-${AWS_ENDPOINT_URL:-http://localhost:4566}}"
REGION="${AWS_REGION:-us-east-1}"
COUNT="${COUNT:-25}"

export AWS_ACCESS_KEY_ID="${AWS_ACCESS_KEY_ID:-test}"
export AWS_SECRET_ACCESS_KEY="${AWS_SECRET_ACCESS_KEY:-test}"
export AWS_REGION="$REGION"
export AWS_DEFAULT_REGION="$REGION"

# Thin wrapper: every call targets the emulator, quiet output, never paginates.
a() { aws --endpoint-url "$ENDPOINT" --region "$REGION" --no-cli-pager --output text "$@"; }

# Run a quick reachability probe before hammering it.
if ! a sts get-caller-identity >/dev/null 2>&1; then
  echo "✗ Cannot reach the emulator at $ENDPOINT — is floci running? (floci start)" >&2
  exit 1
fi

echo "Seeding $ENDPOINT  (region $REGION, COUNT=$COUNT per service)"
echo "  Reminder: floci must run with FLOCI_STORAGE_MODE=persistent (or hybrid/wal)"
echo "  for this data to survive a restart — the default 'memory' mode loses it."
section() { printf '\n▸ %s\n' "$1"; }
tick() { printf '.'; }
done_() { printf ' ✓ %d\n' "$1"; }

pad() { printf '%03d' "$1"; }

# # ── S3 buckets ────────────────────────────────────────────────────────────────
# section "S3 buckets"
# for i in $(seq 1 "$COUNT"); do
#   a s3api create-bucket --bucket "neboto-seed-$(pad "$i")" >/dev/null 2>&1 && tick
# done; done_ "$COUNT"
#
# # ── DynamoDB tables ───────────────────────────────────────────────────────────
# section "DynamoDB tables"
# for i in $(seq 1 "$COUNT"); do
#   a dynamodb create-table \
#     --table-name "neboto-seed-$(pad "$i")" \
#     --attribute-definitions AttributeName=pk,AttributeType=S \
#     --key-schema AttributeName=pk,KeyType=HASH \
#     --billing-mode PAY_PER_REQUEST >/dev/null 2>&1 && tick
# done; done_ "$COUNT"
#
# # ── SQS queues + SNS topics ───────────────────────────────────────────────────
# section "SQS queues"
# for i in $(seq 1 "$COUNT"); do
#   a sqs create-queue --queue-name "neboto-seed-$(pad "$i")" >/dev/null 2>&1 && tick
# done; done_ "$COUNT"
#
# section "SNS topics"
# for i in $(seq 1 "$COUNT"); do
#   a sns create-topic --name "neboto-seed-$(pad "$i")" >/dev/null 2>&1 && tick
# done; done_ "$COUNT"
#
# # ── IAM roles + users ─────────────────────────────────────────────────────────
# ASSUME='{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"ec2.amazonaws.com"},"Action":"sts:AssumeRole"}]}'
# section "IAM roles"
# for i in $(seq 1 "$COUNT"); do
#   a iam create-role --role-name "neboto-seed-$(pad "$i")" \
#     --assume-role-policy-document "$ASSUME" >/dev/null 2>&1 && tick
# done; done_ "$COUNT"
#
# section "IAM users"
# for i in $(seq 1 "$COUNT"); do
#   a iam create-user --user-name "neboto-seed-$(pad "$i")" >/dev/null 2>&1 && tick
# done; done_ "$COUNT"
#
# # ── KMS keys ──────────────────────────────────────────────────────────────────
# section "KMS keys"
# for i in $(seq 1 "$COUNT"); do
#   kid=$(a kms create-key --description "neboto-seed-$(pad "$i")" --query 'KeyMetadata.KeyId' 2>/dev/null)
#   [ -n "$kid" ] && a kms create-alias --alias-name "alias/neboto-seed-$(pad "$i")" \
#     --target-key-id "$kid" >/dev/null 2>&1 && tick
# done; done_ "$COUNT"
#
# # ── Secrets Manager ───────────────────────────────────────────────────────────
# section "Secrets"
# for i in $(seq 1 "$COUNT"); do
#   a secretsmanager create-secret --name "neboto-seed-$(pad "$i")" \
#     --secret-string "s3cr3t-$(pad "$i")" >/dev/null 2>&1 && tick
# done; done_ "$COUNT"
#
# # ── CloudWatch log groups ─────────────────────────────────────────────────────
# section "CloudWatch log groups"
# for i in $(seq 1 "$COUNT"); do
#   a logs create-log-group --log-group-name "/neboto/seed/$(pad "$i")" >/dev/null 2>&1 && tick
# done; done_ "$COUNT"
#
# ── EC2: one VPC + subnet + SG, then bulk instances + volumes ─────────────────
section "EC2 (VPC, subnet, SG, instances, volumes)"
VPC=$(a ec2 create-vpc --cidr-block 10.0.0.0/16 --query 'Vpc.VpcId' 2>/dev/null)
SUBNET=$(a ec2 create-subnet --vpc-id "$VPC" --cidr-block 10.0.1.0/24 --query 'Subnet.SubnetId' 2>/dev/null)
SG=$(a ec2 create-security-group --group-name neboto-seed --description "neboto seed" \
  --vpc-id "$VPC" --query 'GroupId' 2>/dev/null)
AZ="${REGION}a"
for i in $(seq 1 "$COUNT"); do
  a ec2 run-instances --image-id ami-0abcdef1234567890 --instance-type t3.micro \
    --subnet-id "$SUBNET" --security-group-ids "$SG" \
    --tag-specifications "ResourceType=instance,Tags=[{Key=Name,Value=neboto-seed-$(pad "$i")}]" \
    >/dev/null 2>&1 && tick
  a ec2 create-volume --availability-zone "$AZ" --size 8 \
    --tag-specifications "ResourceType=volume,Tags=[{Key=Name,Value=neboto-seed-$(pad "$i")}]" \
    >/dev/null 2>&1
done
done_ "$COUNT"

# ── EKS clusters (optional — heavier; gated behind SEED_EKS=1) ─────────────────
section "EKS clusters"
ROLE_ARN="arn:aws:iam::000000000000:role/neboto-seed-001"
for i in $(seq 1 "$COUNT"); do
  a eks create-cluster --name "neboto-seed-$(pad "$i")" \
    --role-arn "$ROLE_ARN" \
    --resources-vpc-config "subnetIds=$SUBNET" >/dev/null 2>&1 && tick
done
done_ "$COUNT"

echo
echo "Done. Launch neboto against the emulator:"
echo "  AWS_ENDPOINT_URL=$ENDPOINT cargo run"
