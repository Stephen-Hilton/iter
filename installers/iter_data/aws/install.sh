#!/usr/bin/env bash
# Launch an iter_data server on AWS with CloudFormation (iter_data.cfn.yaml beside this
# script): one EC2 Amazon Linux 2023 instance that installs ArangoDB + iter_data at first
# boot from a GitHub Release (installers/iter_data/linux_aws/install.sh), an encrypted EBS
# data volume, an Elastic IP, and the admin password and iter_data's TLS certificate
# fingerprint in SSM Parameter Store. iter_data serves HTTPS with a self-signed
# certificate: engines pin its fingerprint, browsers warn until you accept it.
#
#   ./install.sh --vpc vpc-… --subnet subnet-… --allowed-cidr 203.0.113.0/24 [options]
#   ./install.sh status [--stack-name NAME]     the stack's state and outputs
#   ./install.sh password [--stack-name NAME]   print the first-boot admin password from SSM (gone
#                                               once the admin has chosen their own)
#   ./install.sh fingerprint [--stack-name NAME]  print iter_data's TLS certificate fingerprint from SSM
#                                               (for engine installers: --fingerprint sha256:...)
#   ./install.sh delete [--stack-name NAME]     delete the stack (the data volume is kept as a snapshot)
#   curl -fsSL https://raw.githubusercontent.com/Stephen-Hilton/iter/main/installers/iter_data/aws/install.sh | bash -s -- --vpc … --subnet … --allowed-cidr …
#
# Options:
#   --stack-name NAME      CloudFormation stack name [iter-data]
#   --vpc ID               VPC to run in (required for a new stack)
#   --subnet ID            a public subnet of that VPC (required for a new stack)
#   --allowed-cidr CIDR    who may reach iter_data on its port (required for a new stack)
#   --version TAG          iter release to install, e.g. v2026.10.17 [latest]
#   --arch arm64|x86_64    Graviton or Intel/AMD [arm64]
#   --instance-type TYPE   [t4g.large for arm64, t3.large for x86_64]
#   --volume-gib N         database volume size [50]
#   --port N               iter_data port [8400]
#   --ssh-cidr CIDR        also open SSH to this range (default: none; use Session Manager)
#   --key-name NAME        EC2 key pair for SSH
#   --region R, --profile P   passed to the aws CLI
#   --dry-run              print the aws commands, run nothing
#   -h, --help             this text
#
# Needs: the aws CLI v2 signed in with rights to create EC2, IAM role, EBS, EIP and
# CloudFormation resources. Never needs the iter repository: piped from curl it fetches
# the template from the same ref.
set -euo pipefail

REPO=Stephen-Hilton/iter
FOLDER=installers/iter_data/aws
TEMPLATE=iter_data.cfn.yaml

CMD=deploy STACK=iter-data VPC="" SUBNET="" CIDR="" VERSION=latest ARCH=arm64 ITYPE="" VOL=50 PORT=8400
SSH_CIDR="" KEY="" DRY=0
AWSARGS=()

say() { echo "[iter aws] $*"; }
die() { echo "[iter aws] error: $*" >&2; exit 1; }
usage() { sed -n '2,/^set -euo/p' "${BASH_SOURCE[0]:-/dev/null}" 2>/dev/null | sed -e '$d' -e 's/^# \{0,1\}//' || true; }

case "${1:-}" in deploy|status|password|fingerprint|delete) CMD="$1"; shift ;; esac
while [ $# -gt 0 ]; do
  case "$1" in
    --stack-name) STACK="$2"; shift 2 ;;
    --vpc) VPC="$2"; shift 2 ;;
    --subnet) SUBNET="$2"; shift 2 ;;
    --allowed-cidr) CIDR="$2"; shift 2 ;;
    --version) VERSION="$2"; shift 2 ;;
    --arch) ARCH="$2"; shift 2 ;;
    --instance-type) ITYPE="$2"; shift 2 ;;
    --volume-gib) VOL="$2"; shift 2 ;;
    --port) PORT="$2"; shift 2 ;;
    --ssh-cidr) SSH_CIDR="$2"; shift 2 ;;
    --key-name) KEY="$2"; shift 2 ;;
    --region) AWSARGS+=(--region "$2"); shift 2 ;;
    --profile) AWSARGS+=(--profile "$2"); shift 2 ;;
    --dry-run) DRY=1; shift ;;
    deploy|status|password|fingerprint|delete) CMD="$1"; shift ;;
    -h|--help) usage; exit 0 ;;
    *) die "unknown option $1 (--help)" ;;
  esac
done

case "$ARCH" in arm64|x86_64) ;; *) die "--arch is arm64 or x86_64" ;; esac
[ -n "$ITYPE" ] || { [ "$ARCH" = arm64 ] && ITYPE=t4g.large || ITYPE=t3.large; }
[[ "$VERSION" =~ ^(latest|v[0-9]{4}\.[0-9]{2}\.[0-9]{2}(\.[0-9]+)?)$ ]] || die "--version is latest or vYYYY.MM.DD[.N]"

awsx() { aws ${AWSARGS[@]+"${AWSARGS[@]}"} "$@"; }  # bash 3.2: an empty array is "unbound" under set -u
awsc() {  # print, then run unless --dry-run
  echo "  + aws ${AWSARGS[*]+${AWSARGS[*]} }$*"
  [ "$DRY" = 1 ] || awsx "$@"
}

command -v aws >/dev/null || [ "$DRY" = 1 ] || die "the aws CLI is required (https://aws.amazon.com/cli/)"

param_name() { echo "/iter/$STACK/admin-password"; }
fp_param() { echo "/iter/$STACK/tls-fingerprint"; }

case "$CMD" in
  status)
    awsc cloudformation describe-stacks --stack-name "$STACK" \
      --query 'Stacks[0].[StackStatus, Outputs[].[OutputKey, OutputValue]]' --output text
    exit 0 ;;
  password)
    awsc ssm get-parameter --with-decryption --name "$(param_name)" --query Parameter.Value --output text \
      || die "no $(param_name): the admin has already chosen their own password (the instance then deletes the first-boot one), or the stack is not up yet"
    exit 0 ;;
  fingerprint)
    awsc ssm get-parameter --name "$(fp_param)" --query Parameter.Value --output text
    exit 0 ;;
  delete)
    say "deleting stack $STACK (the database volume is kept as an EBS snapshot)"
    awsc cloudformation delete-stack --stack-name "$STACK"
    awsc cloudformation wait stack-delete-complete --stack-name "$STACK"
    say "deleted. The parameters $(param_name) and $(fp_param) were written by the instance: remove them with"
    say "  aws ssm delete-parameters --names $(param_name) $(fp_param)"
    exit 0 ;;
esac

# deploy: the template beside this script, else fetched from the same ref
if ! HERE="$(cd "$(dirname "${BASH_SOURCE[0]:-.}")" 2>/dev/null && pwd)"; then HERE="$PWD"; fi
TPL="$HERE/$TEMPLATE"
if [ ! -f "$TPL" ]; then
  ref=main; [ "$VERSION" = latest ] || ref="$VERSION"
  TPL="$(mktemp -t iter-data-cfn.XXXXXX)"
  trap 'rm -f "$TPL"' EXIT
  say "template not beside the script: fetching $FOLDER/$TEMPLATE ($ref)"
  curl -fsSL --retry 3 -o "$TPL" "https://raw.githubusercontent.com/$REPO/$ref/$FOLDER/$TEMPLATE" \
    || die "could not fetch the template from $REPO@$ref"
fi

exists=0
if [ "$DRY" = 0 ] && awsx cloudformation describe-stacks --stack-name "$STACK" >/dev/null 2>&1; then exists=1; fi
if [ "$exists" = 0 ]; then
  [ -n "$VPC" ] && [ -n "$SUBNET" ] && [ -n "$CIDR" ] || [ "$DRY" = 1 ] \
    || die "a new stack needs --vpc, --subnet and --allowed-cidr"
fi

params=("IterVersion=$VERSION" "Architecture=$ARCH" "InstanceType=$ITYPE" "DataVolumeSize=$VOL" "IterPort=$PORT"
        "SshCidr=$SSH_CIDR" "KeyName=$KEY")
[ -z "$VPC" ] || params+=("VpcId=$VPC")
[ -z "$SUBNET" ] || params+=("SubnetId=$SUBNET")
[ -z "$CIDR" ] || params+=("AllowedCidr=$CIDR")

say "stack $STACK: $ARCH $ITYPE, iter $VERSION, port $PORT open to ${CIDR:-<unchanged>}"
say "first boot installs ArangoDB + iter_data; the stack completes when /health answers (about 10 minutes)"
awsc cloudformation deploy --stack-name "$STACK" --template-file "$TPL" \
  --capabilities CAPABILITY_IAM --no-fail-on-empty-changeset --parameter-overrides "${params[@]}"
awsc cloudformation describe-stacks --stack-name "$STACK" \
  --query 'Stacks[0].Outputs[].[OutputKey, OutputValue]' --output text
say "admin password: $0 password --stack-name $STACK   (or the AdminPasswordParameter output in SSM)"
say "  you must change it at the first sign-in (the webui asks; a new password needs 12+ characters);"
say "  the instance then deletes it from SSM and from /etc/iter/data.env"
say "TLS certificate fingerprint: $0 fingerprint --stack-name $STACK   (the TlsFingerprintParameter output)"
say "  give it to each engine installer (--fingerprint sha256:...) or compare it with \`iter_engine key status\`;"
say "  the Url is https:// with a self-signed certificate, so browsers warn until you accept it (or put a TLS proxy in front)"
say "note: the stack's parameters only apply at first boot; to move to a newer iter release, run"
say "  sudo installers/iter_data/linux_aws/install.sh install --version <tag> on the instance (Session Manager)."
