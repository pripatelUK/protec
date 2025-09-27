import { useEffect, useRef, useState } from 'react'
import './App.css'

type PairingStartResponse = {
  pairing_code: string
  qr_data: string
  session_id: string
}

type PairingEvent =
  | { type: 'pending' }
  | { type: 'paired'; session_id: string; rpc_endpoint: string }
  | { type: 'expired' }

export default function App() {
  const [resp, setResp] = useState<PairingStartResponse | null>(null)
  const [event, setEvent] = useState<PairingEvent | null>(null)
  const wsRef = useRef<WebSocket | null>(null)

  useEffect(() => {
    return () => {
      wsRef.current?.close()
    }
  }, [])

  async function startPairing() {
    setEvent(null)
    const r = await fetch('http://localhost:3000/api/pairing/start', {
      method: 'POST'
    })
    const j = (await r.json()) as PairingStartResponse
    setResp(j)

    // open WS
    const ws = new WebSocket(`ws://localhost:3000/ws/pairing/${j.pairing_code}`)
    wsRef.current = ws
    ws.onmessage = (m) => {
      try {
        const e = JSON.parse(m.data as string) as PairingEvent
        setEvent(e)
      } catch { }
    }
    ws.onclose = () => {
      wsRef.current = null
    }
  }

  return (
    <div style={{ padding: 24 }}>
      <h1>🔐 Secure Mobile Transaction Approval</h1>
      <p>Phase 1: Pair your device to get a personal RPC endpoint.</p>

      <button onClick={startPairing}>Start Pairing</button>

      {resp && (
        <div style={{ marginTop: 16 }}>
          <h3>Pairing Code</h3>
          <code>{resp.pairing_code}</code>
          <p style={{ marginTop: 8 }}>Scan in mobile app or send this code to it.</p>
          <h3>QR</h3>
          <div style={{ border: '1px dashed #888', padding: 12 }}>
            <small>QR data:</small>
            <div>
              <code>{resp.qr_data}</code>
            </div>
          </div>
        </div>
      )}

      {event && (
        <div style={{ marginTop: 16 }}>
          <h3>Status</h3>
          {event.type === 'pending' && <div>Waiting for device…</div>}
          {event.type === 'expired' && <div>Expired. Start again.</div>}
          {event.type === 'paired' && (
            <div>
              <div>✅ Paired</div>
              <div style={{ marginTop: 8 }}>
                <strong>Your Personal RPC Endpoint:</strong>
                <div style={{ display: 'flex', gap: 8, alignItems: 'center', marginTop: 4 }}>
                  <code>{event.rpc_endpoint}</code>
                  <button onClick={() => navigator.clipboard.writeText(event.rpc_endpoint)}>Copy</button>
                </div>
              </div>
            </div>
          )}
        </div>
      )}
    </div>
  )
}
