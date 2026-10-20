#!/usr/bin/env bash
# Launch an iter_data server on AWS with CloudFormation (iter_data.cfn.yaml beside this
# script): one EC2 Amazon Linux 2023 instance that installs ArangoDB + iter_data at first
# boot from a GitHub Release (installers/iter_data/linux_aws/install.sh), an encrypted EBS
# data volume, an Elastic IP, and the admin password and iter_data's TLS certificate
# fingerprint in SSM Parameter Store. iter_data serves HTTPS with a certificate signed
# by its own local CA: engines pin its fingerprint; a browser trusts it once an engine
# is installed on its machine, or after opening <Url>/tls/ca.crt and trusting it as a CA.
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
#   --iter-dir DIR         the folder on the data volume (/data) that holds everything of
#                          iter_data, its database and backups included; always ends in /iter [/data/iter]
#   --ssh-cidr CIDR        also open SSH to this range (default: none; use Session Manager)
#   --key-name NAME        EC2 key pair for SSH
#   --arango-url URL       use this ArangoDB (https://host:8529) instead of installing one on the
#                          instance [local]. Its passwords come from ARANGO_PASSWORD (iter_data's
#                          own user, never root's) and, only to create that user or database,
#                          ARANGO_ROOT_PASSWORD in this shell's environment: stored as SSM
#                          SecureStrings /iter/<stack>/arango-password and arango-root-password,
#                          never passed through CloudFormation
#   --arango-fingerprint FP  pin the ArangoDB's self-signed certificate (sha256:<64 hex>)
#   --arango-ca-cert PATH  trust this PEM CA for the ArangoDB (stored as /iter/<stack>/arango-ca-cert)
#   --arango-db NAME       iter_data's database [iter_db]
#   --arango-user NAME     the user iter_data signs in as, rw on its database only [iter_user]
#   --arango-allow-plain-http  allow an http:// --arango-url on another machine (a trusted private network)
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

CMD=deploy STACK=iter-data VPC="" SUBNET="" CIDR="" VERSION=latest ARCH=arm64 ITYPE="" VOL=50 PORT=8400 IDIR=""
SSH_CIDR="" KEY="" DRY=0
A_URL="" A_FP="" A_CA="" A_DB="" A_USER="" A_PLAIN=""
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
    --iter-dir|--iter_dir) IDIR="$2"; shift 2 ;;
    --ssh-cidr) SSH_CIDR="$2"; shift 2 ;;
    --key-name) KEY="$2"; shift 2 ;;
    --arango-url) A_URL="$2"; shift 2 ;;
    --arango-fingerprint) A_FP="$2"; shift 2 ;;
    --arango-ca-cert) A_CA="$2"; shift 2 ;;
    --arango-db) A_DB="$2"; shift 2 ;;
    --arango-user) A_USER="$2"; shift 2 ;;
    --arango-allow-plain-http) A_PLAIN=true; shift ;;
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
[[ "$VERSION" =~ ^(latest|v[0-9]{4}\.[0-9]{2}\.[0-9]{2}([a-z]|\.[0-9]+)?)$ ]] || die "--version is latest or a tag like v2026.10.20a"
url_re='^(local|https?://[][A-Za-z0-9._:-]+/?)$'  # host (or [IPv6]) and port
[ -z "$A_URL" ] || [[ "$A_URL" =~ $url_re ]] || die "--arango-url is https://host:port (or local)"
[ -z "$A_FP" ] || [[ "$A_FP" =~ ^(sha256:)?[0-9A-Fa-f:]{64,95}$ ]] || die "--arango-fingerprint is sha256:<64 hex>"
[ -z "$A_DB$A_USER" ] || [[ "$A_DB$A_USER" =~ ^[A-Za-z0-9_-]+$ ]] || die "--arango-db / --arango-user: letters, digits, _ and - only"
[ "${A_USER:-}" != root ] || die "--arango-user root: iter_data signs in as a user of its own, with rw on its database only"
[ -z "$A_CA" ] || [ -f "$A_CA" ] || die "--arango-ca-cert $A_CA does not exist"
EXTERNAL=0; [ -z "$A_URL" ] || [ "$A_URL" = local ] || EXTERNAL=1
[ "$EXTERNAL" = 1 ] || [ -z "$A_FP$A_CA$A_PLAIN" ] || die "--arango-fingerprint/--arango-ca-cert/--arango-allow-plain-http need --arango-url"
if [ "$EXTERNAL" = 1 ] && [[ "$A_URL" == http://* ]] && [ -z "$A_PLAIN" ]; then
  die "--arango-url $A_URL is plain http: use https:// (with --arango-fingerprint or --arango-ca-cert), or add --arango-allow-plain-http on a private network"
fi
[ -z "${ARANGO_PASSWORD:-}" ] || [ "${ARANGO_PASSWORD}" != "${ARANGO_ROOT_PASSWORD:-}" ] \
  || die "ARANGO_PASSWORD is root's password: give iter_data's user a password of its own"

awsx() { aws ${AWSARGS[@]+"${AWSARGS[@]}"} "$@"; }  # bash 3.2: an empty array is "unbound" under set -u
awsc() {  # print, then run unless --dry-run
  echo "  + aws ${AWSARGS[*]+${AWSARGS[*]} }$*"
  [ "$DRY" = 1 ] || awsx "$@"
}

command -v aws >/dev/null || [ "$DRY" = 1 ] || die "the aws CLI is required (https://aws.amazon.com/cli/)"

param_name() { echo "/iter/$STACK/admin-password"; }
fp_param() { echo "/iter/$STACK/tls-fingerprint"; }
# an SSM SecureString from an environment variable; the value is never printed
put_secret() {  # NAME VAR
  echo "  + aws ${AWSARGS[*]+${AWSARGS[*]} }ssm put-parameter --type SecureString --overwrite --name /iter/$STACK/$1 --value <\$$2>"
  [ "$DRY" = 1 ] || awsx ssm put-parameter --type SecureString --overwrite --name "/iter/$STACK/$1" --value "${!2}" >/dev/null
}

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
    say "  (with an external ArangoDB, also /iter/$STACK/arango-password, arango-root-password and arango-ca-cert)"
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
[ -z "$A_URL" ] || params+=("ArangoUrl=$A_URL")
[ -z "$A_FP" ] || params+=("ArangoCertFingerprint=$A_FP")
[ -z "$A_DB" ] || params+=("ArangoDb=$A_DB")
[ -z "$A_USER" ] || params+=("ArangoUser=$A_USER")
[ -z "$A_PLAIN" ] || params+=("ArangoAllowPlainHttp=true")
if [ -n "$IDIR" ]; then
  # forgiving like the install scripts: /data/x, /data/x/iter and /data/x/iter/ all mean /data/x/iter
  while [ "${#IDIR}" -gt 1 ] && [ "${IDIR%/}" != "$IDIR" ]; do IDIR="${IDIR%/}"; done
  [ "${IDIR##*/}" = iter ] || IDIR="$IDIR/iter"
  case "$IDIR" in /data/*) params+=("IterDir=$IDIR") ;; *) die "--iter-dir $IDIR: a folder on the data volume, /data/..." ;; esac
fi

# what the first boot reads from SSM for an external ArangoDB (before the stack, so it is there)
if [ -n "${ARANGO_PASSWORD:-}" ]; then put_secret arango-password ARANGO_PASSWORD; fi
if [ -n "${ARANGO_ROOT_PASSWORD:-}" ]; then put_secret arango-root-password ARANGO_ROOT_PASSWORD; fi
if [ -n "$A_CA" ]; then
  awsc ssm put-parameter --type String --tier Intelligent-Tiering --overwrite --name "/iter/$STACK/arango-ca-cert" --value "file://$A_CA"
fi
if [ "$EXTERNAL" = 1 ] && [ -z "${ARANGO_PASSWORD:-}" ]; then
  say "no ARANGO_PASSWORD in the environment: the instance uses /iter/$STACK/arango-password if it exists, else makes one (then ARANGO_ROOT_PASSWORD must create the user)"
fi

say "stack $STACK: $ARCH $ITYPE, iter $VERSION, port $PORT open to ${CIDR:-<unchanged>}"
if [ "$EXTERNAL" = 1 ]; then say "ArangoDB: $A_URL (not installed on the instance)"; fi
say "first boot installs iter_data (and ArangoDB, unless --arango-url); the stack completes when /health answers (about 10 minutes)"
awsc cloudformation deploy --stack-name "$STACK" --template-file "$TPL" \
  --capabilities CAPABILITY_IAM --no-fail-on-empty-changeset --parameter-overrides "${params[@]}"
awsc cloudformation describe-stacks --stack-name "$STACK" \
  --query 'Stacks[0].Outputs[].[OutputKey, OutputValue]' --output text
say "admin password: $0 password --stack-name $STACK   (or the AdminPasswordParameter output in SSM)"
say "  you must change it at the first sign-in (the webui asks; a new password needs 12+ characters);"
say "  the instance then deletes it from SSM and from iter_data/config/data.env"
say "TLS certificate fingerprint: $0 fingerprint --stack-name $STACK   (the TlsFingerprintParameter output)"
say "  give it to each engine installer (--fingerprint sha256:...) or compare it with \`iter_engine key status\`;"
say "  the Url is https:// with a certificate signed by iter_data's local CA: browsers on a machine with an engine trust it"
say "  (its installer adds the CA); elsewhere open <Url>/tls/ca.crt and trust it as a certificate authority"
say "note: the stack's parameters only apply at first boot; to move to a newer iter release, run"
say "  sudo installers/iter_data/linux_aws/install.sh install --version <tag> on the instance (Session Manager)."
