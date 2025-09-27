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

log "Preview intent: eth_estimateGas (creates Pending approval; upstream may error, which is fine)"
$CURL_BIN -sS -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":10,"method":"eth_estimateGas","params":[{"from":"0x0000000000000000000000000000000000000001","to":"0x0000000000000000000000000000000000000002","value":"0x0","data":"0x"}]}' \
  "$RPC_URL" | $JQ_BIN .

log "Final gate: eth_sendRawTransaction (intent likely mismatched; expect approval error or not-found)"
SEND_RES=$($CURL_BIN -sS -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":11,"method":"eth_sendRawTransaction","params":["0x01"]}' \
  "$RPC_URL")
echo "$SEND_RES" | $JQ_BIN .
CODE=$(echo "$SEND_RES" | $JQ_BIN -r '.error.code // empty')
if [[ -n "$CODE" ]]; then
  log "Gate responded with error code: $CODE (expected -32001 approval required if intents match, or -32004 no approval if not)"
else
  log "Gate forwarded upstream (no gating applied)"
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


