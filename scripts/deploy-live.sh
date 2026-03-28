#!/usr/bin/env bash
# FILE: scripts/deploy-live.sh
# VERSION: 0.1.0
# START_MODULE_CONTRACT
#   PURPOSE: Stage or update the governed n0wss artifact set on one managed host using the canonical Phase-9 layout.
#   SCOPE: Argument validation, role-aware asset checks, canonical remote path preparation, bounded dry-run output, and atomic remote switchover.
#   DEPENDS: bash, ssh, scp, sha256sum
#   LINKS: M-DEPLOY-SCRIPTING, M-DEPLOY-LAYOUT, M-SECRET-HYGIENE, V-M-DEPLOY-SCRIPTING, DF-DEPLOY-PACKAGE
# END_MODULE_CONTRACT
#
# START_MODULE_MAP
#   print_usage - show the governed CLI surface for managed deployment
#   require_file - fail fast when a required local artifact is missing
#   deploy_file - upload one artifact via a temporary path and atomically switch it into place
#   deploy_server_role - stage server-role artifacts under the canonical layout
#   deploy_client_role - stage client-role artifacts under the canonical layout
# END_MODULE_MAP
#
# START_CHANGE_SUMMARY
#   LAST_CHANGE: v0.1.0 - Added the first managed deployment script with dry-run support, canonical path enforcement, and role-aware artifact staging.
# END_CHANGE_SUMMARY

set -euo pipefail

ROLE=""
HOST=""
SSH_USER="root"
SSH_PORT="22"
REMOTE_ROOT="/opt/n0wss"
DRY_RUN="false"
BINARY_PATH=""
ENV_FILE=""
SERVER_CERT=""
SERVER_KEY=""
TRUST_ANCHOR=""

print_usage() {
  cat <<'EOF'
Usage:
  scripts/deploy-live.sh --role server|client --host HOST --binary PATH --env-file PATH [options]

Options:
  --dry-run                  Print the planned actions without changing the remote host
  --ssh-user USER           SSH user (default: root)
  --ssh-port PORT           SSH port (default: 22)
  --remote-root PATH        Canonical remote root (default: /opt/n0wss)
  --server-cert PATH        Required for server role
  --server-key PATH         Required for server role
  --trust-anchor PATH       Required for client role
EOF
}

require_file() {
  local path="$1"
  local label="$2"

  # START_BLOCK_VALIDATE_LOCAL_INPUT
  if [[ ! -f "$path" ]]; then
    echo "[DeployScripting][validateInputs][BLOCK_VALIDATE_LOCAL_INPUT] missing ${label}: ${path}" >&2
    exit 1
  fi
  # END_BLOCK_VALIDATE_LOCAL_INPUT
}

remote_exec() {
  ssh -p "$SSH_PORT" -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null "${SSH_USER}@${HOST}" "$@"
}

deploy_file() {
  local local_path="$1"
  local remote_path="$2"
  local remote_tmp="${remote_path}.tmp"
  local remote_mode="$3"

  # START_BLOCK_DEPLOY_SWITCHOVER
  echo "[DeployScripting][deployFile][BLOCK_DEPLOY_SWITCHOVER] staging ${local_path} -> ${HOST}:${remote_path}"

  if [[ "$DRY_RUN" == "true" ]]; then
    return 0
  fi

  scp -P "$SSH_PORT" -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null \
    "$local_path" "${SSH_USER}@${HOST}:${remote_tmp}"

  remote_exec "install -D -m ${remote_mode} '${remote_tmp}' '${remote_path}' && rm -f '${remote_tmp}'"
  # END_BLOCK_DEPLOY_SWITCHOVER
}

prepare_remote_layout() {
  local env_dir="${REMOTE_ROOT}/env"
  local cert_dir="${REMOTE_ROOT}/certs"
  local run_dir="${REMOTE_ROOT}/run"

  echo "[DeployScripting][prepareRemoteLayout][BLOCK_DEPLOY_SWITCHOVER] ensuring canonical layout on ${HOST}"

  if [[ "$DRY_RUN" == "true" ]]; then
    return 0
  fi

  remote_exec "install -d -m 0755 '${REMOTE_ROOT}' '${env_dir}' '${cert_dir}' '${run_dir}'"
}

deploy_server_role() {
  require_file "$BINARY_PATH" "binary"
  require_file "$ENV_FILE" "server env file"
  require_file "$SERVER_CERT" "server certificate"
  require_file "$SERVER_KEY" "server key"

  prepare_remote_layout
  deploy_file "$BINARY_PATH" "${REMOTE_ROOT}/n0wss" "0755"
  deploy_file "$ENV_FILE" "${REMOTE_ROOT}/env/server.env" "0600"
  deploy_file "$SERVER_CERT" "${REMOTE_ROOT}/certs/server.pem" "0644"
  deploy_file "$SERVER_KEY" "${REMOTE_ROOT}/certs/server.key" "0600"
}

deploy_client_role() {
  require_file "$BINARY_PATH" "binary"
  require_file "$ENV_FILE" "client env file"
  require_file "$TRUST_ANCHOR" "client trust anchor"

  prepare_remote_layout
  deploy_file "$BINARY_PATH" "${REMOTE_ROOT}/n0wss" "0755"
  deploy_file "$ENV_FILE" "${REMOTE_ROOT}/env/client.env" "0600"
  deploy_file "$TRUST_ANCHOR" "${REMOTE_ROOT}/certs/server.pem" "0644"
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --role)
      ROLE="${2:-}"
      shift 2
      ;;
    --host)
      HOST="${2:-}"
      shift 2
      ;;
    --ssh-user)
      SSH_USER="${2:-}"
      shift 2
      ;;
    --ssh-port)
      SSH_PORT="${2:-}"
      shift 2
      ;;
    --remote-root)
      REMOTE_ROOT="${2:-}"
      shift 2
      ;;
    --binary)
      BINARY_PATH="${2:-}"
      shift 2
      ;;
    --env-file)
      ENV_FILE="${2:-}"
      shift 2
      ;;
    --server-cert)
      SERVER_CERT="${2:-}"
      shift 2
      ;;
    --server-key)
      SERVER_KEY="${2:-}"
      shift 2
      ;;
    --trust-anchor)
      TRUST_ANCHOR="${2:-}"
      shift 2
      ;;
    --dry-run)
      DRY_RUN="true"
      shift
      ;;
    --help|-h)
      print_usage
      exit 0
      ;;
    *)
      echo "Unknown argument: $1" >&2
      print_usage >&2
      exit 1
      ;;
  esac
done

if [[ -z "$ROLE" || -z "$HOST" || -z "$BINARY_PATH" || -z "$ENV_FILE" ]]; then
  print_usage >&2
  exit 1
fi

if [[ "$ROLE" != "server" && "$ROLE" != "client" ]]; then
  echo "Unsupported role: ${ROLE}" >&2
  exit 1
fi

echo "[DeployScripting][main][BLOCK_DEPLOY_SWITCHOVER] role=${ROLE} host=${HOST} remote_root=${REMOTE_ROOT} dry_run=${DRY_RUN}"
echo "[DeployScripting][main][BLOCK_DEPLOY_SWITCHOVER] artifact_sha256=$(sha256sum "$BINARY_PATH" | awk '{print $1}')"

if [[ "$ROLE" == "server" ]]; then
  deploy_server_role
else
  deploy_client_role
fi

echo "[DeployScripting][main][BLOCK_DEPLOY_SWITCHOVER] completed role=${ROLE} host=${HOST}"
