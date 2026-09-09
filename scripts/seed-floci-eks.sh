#!/usr/bin/env bash
#
# Minimal EKS seed for floci: one VPC + subnet + security group, an IAM role,
# and exactly ONE EKS cluster. Use this instead of `SEED_EKS=1 seed-floci.sh`
# (which loops COUNT times) — each EKS cluster spins up a REAL Kubernetes
# container in Docker, so creating many will pin your CPU. One is enough to
# exercise neboto's EKS list + Details pane.
#
# Usage:
#   FLOCI_STORAGE_MODE=persistent FLOCI_STORAGE_PERSISTENT_PATH="$HOME/.floci-data" floci start
#   ./scripts/seed-floci-eks.sh
#   AWS_ENDPOINT_URL=http://localhost:4566 cargo run     # then open @eks
#
# Note: floci serves the EKS control plane (list/describe-cluster) but NOT the
# sub-resource endpoints (node-groups / fargate-profiles / addons) — those
# sections will show an "unavailable" error against floci. The cluster list and
# the Details section are what you can test here.
#
# Requires the `aws` CLI. Credentials are dummy — the emulator ignores them.

set -uo pipefail

ENDPOINT="${ENDPOINT:-${AWS_ENDPOINT_URL:-http://localhost:4566}}"
REGION="${AWS_REGION:-us-east-1}"
NAME="${EKS_NAME:-neboto-eks-1}"

export AWS_ACCESS_KEY_ID="${AWS_ACCESS_KEY_ID:-test}"
export AWS_SECRET_ACCESS_KEY="${AWS_SECRET_ACCESS_KEY:-test}"
export AWS_REGION="$REGION"
export AWS_DEFAULT_REGION="$REGION"

a() { aws --endpoint-url "$ENDPOINT" --region "$REGION" --no-cli-pager --output text "$@"; }

if ! a sts get-caller-identity >/dev/null 2>&1; then
  echo "✗ Cannot reach the emulator at $ENDPOINT — is floci running? (floci start)" >&2
  exit 1
fi

echo "Seeding one EKS cluster at $ENDPOINT (region $REGION)"

# ── VPC + subnet + security group ─────────────────────────────────────────────
echo "▸ VPC / subnet / security group"
VPC=$(a ec2 create-vpc --cidr-block 10.0.0.0/16 --query 'Vpc.VpcId' 2>/dev/null)
SUBNET=$(a ec2 create-subnet --vpc-id "$VPC" --cidr-block 10.0.1.0/24 \
  --availability-zone "${REGION}a" --query 'Subnet.SubnetId' 2>/dev/null)
SG=$(a ec2 create-security-group --group-name neboto-eks --description "neboto eks" \
  --vpc-id "$VPC" --query 'GroupId' 2>/dev/null)
echo "  VPC=$VPC  SUBNET=$SUBNET  SG=$SG"

# ── Cluster service role ──────────────────────────────────────────────────────
echo "▸ IAM cluster role"
ASSUME='{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"eks.amazonaws.com"},"Action":"sts:AssumeRole"}]}'
ROLE_ARN=$(a iam create-role --role-name neboto-eks-role \
  --assume-role-policy-document "$ASSUME" --query 'Role.Arn' 2>/dev/null)
# Fall back to a synthetic ARN if the role already exists / floci stubs it.
ROLE_ARN="${ROLE_ARN:-arn:aws:iam::000000000000:role/neboto-eks-role}"
echo "  ROLE=$ROLE_ARN"

# ── One EKS cluster ───────────────────────────────────────────────────────────
echo "▸ EKS cluster: $NAME"
a eks create-cluster --name "$NAME" \
  --role-arn "$ROLE_ARN" \
  --resources-vpc-config "subnetIds=$SUBNET,securityGroupIds=$SG" \
  >/dev/null 2>&1 \
  && echo "  ✓ requested" \
  || echo "  ✗ create-cluster failed (see: a eks create-cluster ... without 2>/dev/null)"

echo
echo "Done. Open neboto and go to @eks:"
echo "  AWS_ENDPOINT_URL=$ENDPOINT cargo run"
