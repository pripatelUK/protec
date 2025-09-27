# Mobile App

Minimal Flutter client for pairing with the backend.

## Local Development Notes

- Backend must listen on `0.0.0.0:3000` (already configured).
- When testing on a real Android device via USB, use adb reverse to route the phone’s `127.0.0.1:3000` to your host:

```bash
adb reverse tcp:3000 tcp:3000
```

- In the app, use `127.0.0.1` as the server host. Example POST (from app logs):

```
POST http://127.0.0.1:3000/api/pair
```

- If testing over Wi‑Fi (no USB), ensure the phone and Mac share the same network, firewall allows inbound 3000, and use the Mac’s LAN IP, e.g. `http://192.168.x.x:3000/health`.
