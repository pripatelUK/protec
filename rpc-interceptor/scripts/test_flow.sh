#!/usr/bin/env bash
set -euo pipefail

# Quick end-to-end test of the pairing + RPC flow without frontend/mobile.
# Requirements: curl, jq

BASE_URL="${BASE_URL:-http://localhost:3000}"
DEVICE_ID="${DEVICE_ID:-device-cli}"
JQ_BIN="${JQ_BIN:-jq}"
CURL_BIN="${CURL_BIN:-curl}"

log() { echo "[$(date '+%H:%M:%S')] $*"; }

require() { command -v "$1" >/dev/null 2>&1 || { echo "Missing dependency: $1"; exit 1; }; }

require "$CURL_BIN"
require "$JQ_BIN"
if command -v cast >/dev/null 2>&1; then
  CAST_BIN="${CAST_BIN:-cast}"
else
  CAST_BIN=""
fi

# Load .env (prefer project root next to this script)
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ENV_PATH="${SCRIPT_DIR}/../.env"
if [[ -f "$ENV_PATH" ]]; then
  log "Loading env from $ENV_PATH"
  set -a
  # shellcheck disable=SC1091
  source "$ENV_PATH"
  set +a
elif [[ -f ./.env ]]; then
  log "Loading env from ./ .env"
  set -a
  # shellcheck disable=SC1091
  source ./.env
  set +a
else
  log "No .env found at $ENV_PATH or ./ .env"
fi

log "Health check $BASE_URL/health"
$CURL_BIN -sS "$BASE_URL/health" | sed -e 's/^/  /'

log "Start pairing"
START_JSON=$($CURL_BIN -sS -X POST "$BASE_URL/api/pairing/start")
PAIRING_CODE=$(echo "$START_JSON" | $JQ_BIN -r '.pairing_code')
SESSION_ID=$(echo "$START_JSON" | $JQ_BIN -r '.session_id')
QR_DATA=$(echo "$START_JSON" | $JQ_BIN -r '.qr_data')
if [[ "$PAIRING_CODE" == "null" || -z "$PAIRING_CODE" ]]; then
  echo "Failed to start pairing: $START_JSON" >&2
  exit 1
fi
log "pairing_code=$PAIRING_CODE session_id=$SESSION_ID qr_data=$QR_DATA"

log "Complete pairing as device_id=$DEVICE_ID"
PAIR_RES=$($CURL_BIN -sS \
  -H 'Content-Type: application/json' \
  -d "{\"pairing_code\":\"$PAIRING_CODE\",\"device_id\":\"$DEVICE_ID\"}" \
  "$BASE_URL/api/pair")
OK=$(echo "$PAIR_RES" | $JQ_BIN -r '.ok')
if [[ "$OK" != "true" ]]; then
  log "Pair response: $PAIR_RES"
  exit 1
fi
RPC_ENDPOINT=$(echo "$PAIR_RES" | $JQ_BIN -r '.rpc_endpoint')
log "Paired. rpc_endpoint=$RPC_ENDPOINT"

log "Check pairing status"
$CURL_BIN -sS "$BASE_URL/pairing/status/$PAIRING_CODE" | $JQ_BIN .

RPC_URL="$RPC_ENDPOINT"
log "RPC eth_chainId"
$CURL_BIN -sS -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"eth_chainId","params":[]}' \
  "$RPC_URL" | $JQ_BIN .

log "RPC eth_blockNumber"
$CURL_BIN -sS -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":2,"method":"eth_blockNumber","params":[]}' \
  "$RPC_URL" | $JQ_BIN .

export TEST_PRIVATE_KEY TEST_WALLET_ADDRESS
HAVE_CAST=0; [[ -n "${CAST_BIN}" ]] && HAVE_CAST=1
HAVE_PK=0;   [[ -n "${TEST_PRIVATE_KEY:-}" ]] && HAVE_PK=1
HAVE_ADDR=0; [[ -n "${TEST_WALLET_ADDRESS:-}" ]] && HAVE_ADDR=1

# Debug print of env detection (mask private key)
MASKED_PK="<unset>"; if [[ $HAVE_PK -eq 1 ]]; then
  PK_LEN=${#TEST_PRIVATE_KEY}
  MASKED_PK="${TEST_PRIVATE_KEY:0:6}...${TEST_PRIVATE_KEY: -4} (len=$PK_LEN)"
fi
log "detect cast=$HAVE_CAST pk_set=$HAVE_PK addr_set=$HAVE_ADDR"
log "TEST_PRIVATE_KEY=$MASKED_PK"
log "TEST_WALLET_ADDRESS=${TEST_WALLET_ADDRESS:-<unset>}"

if [[ $HAVE_CAST -eq 1 && $HAVE_PK -eq 1 && $HAVE_ADDR -eq 1 ]]; then
  log "Preview intent via eth_estimateGas using TEST_PRIVATE_KEY and TEST_WALLET_ADDRESS"
  FROM="$TEST_WALLET_ADDRESS"
  TO=${TEST_TO:-0x0000000000000000000000000000000000000002}
  PREVIEW_PAYLOAD=$(jq -n --arg from "$FROM" --arg to "$TO" '{jsonrpc:"2.0",id:10,method:"eth_estimateGas",params:[{from:$from,to:$to,value:"0x0",data:"0x"}]}')
  echo "$PREVIEW_PAYLOAD" | $CURL_BIN -sS -H 'Content-Type: application/json' -d @- "$RPC_URL" | $JQ_BIN .

  log "Final gate via cast send (should return approval required if intents match)"
  # Attempt to send a 0 ETH tx to TO; cast will sign with TEST_PRIVATE_KEY and call eth_sendRawTransaction
  NO_COLOR=1 SEND_OUT=$($CAST_BIN send --rpc-url "$RPC_URL" --private-key "$TEST_PRIVATE_KEY" "$TO" --value 0 --legacy 2>&1 || true)
  echo "$SEND_OUT"
  if echo "$SEND_OUT" | grep -qE 'code"?:\s*-32001|error code -32001'; then
    log "Gate enforced: received -32001 Mobile approval required (intents matched)"

    # Extract approval_id from error string
    APPROVAL_ID=$(printf "%s" "$SEND_OUT" | tr -d '\n' | grep -oE '"approval_id":"[a-f0-9-]+"' | head -1 | cut -d '"' -f4)
    if [[ -n "$APPROVAL_ID" ]]; then
      log "Approving approval_id=$APPROVAL_ID"
      APPROVE_RES=$($CURL_BIN -sS -X POST "$BASE_URL/api/approve/$APPROVAL_ID")
      echo "$APPROVE_RES" | $JQ_BIN . || true
      OK=$(echo "$APPROVE_RES" | $JQ_BIN -r '.ok' 2>/dev/null || echo "false")
      if [[ "$OK" == "true" ]]; then
        log "Approved. Retrying send (should now pass the gate)"
        NO_COLOR=1 SEND_OUT2=$($CAST_BIN send --rpc-url "$RPC_URL" --private-key "$TEST_PRIVATE_KEY" "$TO" --value 0 --legacy 2>&1 || true)
        echo "$SEND_OUT2"
        if echo "$SEND_OUT2" | grep -qE 'code"?:\s*-32001|error code -32001'; then
          log "Unexpected: gate still enforced after approval"
        else
          log "Gate cleared: no -32001 on retry"
        fi
      else
        log "Approve endpoint did not return ok=true"
      fi
    else
      log "Could not extract approval_id from error output"
    fi
  else
    log "Note: did not observe -32001; check that RPC_PROVIDER_URL chainId matches and params align"
  fi
else
  log "Skipping intent verification: missing dependencies"
  if [[ $HAVE_CAST -ne 1 ]]; then echo "  - cast not installed (brew install foundryup && foundryup)"; fi
  if [[ $HAVE_PK -ne 1 ]]; then echo "  - TEST_PRIVATE_KEY not set in .env"; fi
  if [[ $HAVE_ADDR -ne 1 ]]; then echo "  - TEST_WALLET_ADDRESS not set in .env"; fi
fi

log "RPC invalid session must be rejected (401)"
INVALID_URL="$BASE_URL/rpc/invalid-session-id"
HTTP_CODE=$($CURL_BIN -sS -o /dev/null -w "%{http_code}" -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":99,"method":"eth_chainId","params":[]}' \
  "$INVALID_URL")
echo "  http_code=$HTTP_CODE"
if [[ "$HTTP_CODE" != "401" ]]; then
  echo "Expected 401 for invalid session, got $HTTP_CODE" >&2
  exit 1
fi

log "OK"


