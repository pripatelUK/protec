import 'dart:convert';
import 'dart:io' show Platform;
import 'package:flutter/material.dart';
import 'package:http/http.dart' as http;
import 'package:passkeys/authenticator.dart';
import 'package:passkeys/types.dart';
import 'package:shared_preferences/shared_preferences.dart';
import 'qr_scan_page.dart';

class PairingScreen extends StatefulWidget {
  const PairingScreen({super.key});

  @override
  State<PairingScreen> createState() => _PairingScreenState();
}

class _PairingScreenState extends State<PairingScreen> {
  final TextEditingController pairingCodeController = TextEditingController();
  bool isSubmitting = false;
  bool isBusy = false;

  static const _prefsDeviceIdKey = 'device_id';
  static const _prefsSessionIdKey = 'session_id';
  static const _prefsRpcEndpointKey = 'rpc_endpoint';
  final PasskeyAuthenticator _auth = PasskeyAuthenticator(debugMode: true);

  String get _apiBaseHost => (Platform.isAndroid ? '127.0.0.1' : '127.0.0.1');
  Uri _api(String path) => Uri.parse('http://$_apiBaseHost:3000$path');

  String? _sessionId;
  String? _rpcEndpoint;

  @override
  void initState() {
    super.initState();
    _loadPersistedSession();
  }

  Future<void> _loadPersistedSession() async {
    final prefs = await SharedPreferences.getInstance();
    setState(() {
      _sessionId = prefs.getString(_prefsSessionIdKey);
      _rpcEndpoint = prefs.getString(_prefsRpcEndpointKey);
    });
  }

  Future<void> _persistSession(String sessionId, String rpcEndpoint) async {
    final prefs = await SharedPreferences.getInstance();
    await prefs.setString(_prefsSessionIdKey, sessionId);
    await prefs.setString(_prefsRpcEndpointKey, rpcEndpoint);
    setState(() {
      _sessionId = sessionId;
      _rpcEndpoint = rpcEndpoint;
    });
  }

  Future<String?> _loadStoredDeviceId() async {
    final prefs = await SharedPreferences.getInstance();
    return prefs.getString(_prefsDeviceIdKey);
  }

  Future<void> _storeDeviceId(String deviceId) async {
    final prefs = await SharedPreferences.getInstance();
    await prefs.setString(_prefsDeviceIdKey, deviceId);
  }

  Future<String> _ensureDeviceId() async {
    final existing = await _loadStoredDeviceId();
    if (existing != null && existing.isNotEmpty) return existing;
    final generated = 'device-${DateTime.now().millisecondsSinceEpoch}';
    await _storeDeviceId(generated);
    return generated;
  }

  Future<bool> _resumeWithToken(String assertionToken) async {
    try {
      final res = await http
          .post(
            _api('/api/sessions/resume'),
            headers: {'Content-Type': 'application/json'},
            body: jsonEncode({'assertion_token': assertionToken}),
          )
          .timeout(const Duration(seconds: 10));
      if (res.statusCode != 200) return false;
      final j = jsonDecode(res.body) as Map<String, dynamic>;
      if (j['ok'] == true) {
        final sid = j['session_id'] as String;
        final ep = j['rpc_endpoint'] as String;
        await _persistSession(sid, ep);
        return true;
      }
      return false;
    } catch (_) {
      return false;
    }
  }

  Future<void> _showAlert(String title, String message) async {
    if (!mounted) return;
    await showDialog<void>(
      context: context,
      builder: (context) => AlertDialog(
        title: Text(title),
        content: Text(message),
        actions: [
          TextButton(onPressed: () => Navigator.of(context).pop(), child: const Text('OK')),
        ],
      ),
    );
  }

  Future<void> _registerPasskey() async {
    if (isBusy) return;
    final code = pairingCodeController.text.trim().toLowerCase();
    if (code.isEmpty) {
      await _showAlert('Hint', 'You can register passkey anytime — pairing code not required');
    }
    setState(() => isBusy = true);
    try {
      final deviceId = await _ensureDeviceId();
      final startRes = await http
          .post(
            _api('/api/passkeys/register/start'),
            headers: {'Content-Type': 'application/json'},
            body: jsonEncode({
              'device_id': deviceId,
              'display_name': Platform.isIOS ? 'iOS Device' : 'Android Device',
            }),
          )
          .timeout(const Duration(seconds: 10));
      if (startRes.statusCode != 200) {
        await _showAlert('Error', 'Register/start failed (${startRes.statusCode})');
        return;
      }
      final options = jsonDecode(startRes.body) as Map<String, dynamic>;
      final pk = (options['publicKey'] as Map<String, dynamic>?) ?? options;
      final rp = pk['rp'] as Map<String, dynamic>?;
      final challenge = pk['challenge'] as String;
      final displayName = Platform.isIOS ? 'iOS Device' : 'Android Device';
      final userIdPadded = base64Url.encode(utf8.encode(deviceId));

      final req = RegisterRequestType(
        challenge: challenge,
        relyingParty: RelyingPartyType(
          name: (rp?['name'] as String?) ?? 'Protec',
          id: (rp?['id'] as String?) ?? _apiBaseHost,
        ),
        user: UserType(
          displayName: displayName,
          name: deviceId,
          id: userIdPadded,
        ),
        excludeCredentials: const [],
        pubKeyCredParams: null,
        timeout: 60000,
        attestation: 'none',
      );

      final reg = await _auth.register(req);

      final finishRes = await http
          .post(
            _api('/api/passkeys/register/finish'),
            headers: {'Content-Type': 'application/json'},
            body: jsonEncode({
              'device_id': deviceId,
              'id': reg.id,
              'rawId': reg.rawId,
              'type': 'public-key',
              'response': {
                'attestationObject': reg.attestationObject,
                'clientDataJSON': reg.clientDataJSON,
              }
            }),
          )
          .timeout(const Duration(seconds: 10));
      if (finishRes.statusCode == 200) {
        await _storeDeviceId(deviceId);
        await _showAlert('Passkey ready', 'Passkey registered for this device');
      } else {
        await _showAlert('Error', 'Register/finish failed (${finishRes.statusCode})');
      }
    } catch (e) {
      await _showAlert('Passkey error', '$e');
    } finally {
      if (mounted) setState(() => isBusy = false);
    }
  }

  Future<bool> _signInWithPasskey() async {
    final deviceId = await _loadStoredDeviceId();
    if (deviceId == null) {
      await _showAlert('Not registered', 'Register a passkey on this device first');
      return false;
    }
    try {
      final startRes = await http
          .post(
            _api('/api/passkeys/assert/start'),
            headers: {'Content-Type': 'application/json'},
            body: jsonEncode({'device_id': deviceId}),
          )
          .timeout(const Duration(seconds: 10));
      if (startRes.statusCode != 200) {
        await _showAlert('Error', 'Assert/start failed (${startRes.statusCode})');
        return false;
      }
      final requestOptions = jsonDecode(startRes.body) as Map<String, dynamic>;
      final pk = (requestOptions['publicKey'] as Map<String, dynamic>?) ?? requestOptions;
      final rpId = pk['rpId'] as String;
      final challenge = pk['challenge'] as String;
      final allow = (pk['allowCredentials'] as List<dynamic>? ?? [])
          .map((e) => e as Map<String, dynamic>)
          .map((m) => CredentialType(type: 'public-key', id: m['id'] as String, transports: const []))
          .toList();

      final authReq = AuthenticateRequestType(
        relyingPartyId: rpId,
        challenge: challenge,
        mediation: MediationType.Required,
        preferImmediatelyAvailableCredentials: false,
        timeout: 60000,
        userVerification: 'required',
        allowCredentials: allow.isEmpty ? null : allow,
      );

      final assertion = await _auth.authenticate(authReq);

      final finishRes = await http
          .post(
            _api('/api/passkeys/assert/finish'),
            headers: {'Content-Type': 'application/json'},
            body: jsonEncode({
              'device_id': deviceId,
              'id': assertion.id,
              'rawId': assertion.rawId,
              'type': 'public-key',
              'response': {
                'authenticatorData': assertion.authenticatorData,
                'clientDataJSON': assertion.clientDataJSON,
                'signature': assertion.signature,
                'userHandle': assertion.userHandle,
              }
            }),
          )
          .timeout(const Duration(seconds: 10));
      if (finishRes.statusCode != 200) {
        await _showAlert('Error', 'Assert/finish failed (${finishRes.statusCode})');
        return false;
      }
      final j = jsonDecode(finishRes.body) as Map<String, dynamic>;
      final token = j['assertion_token'] as String?;
      if (token == null) return false;
      final resumed = await _resumeWithToken(token);
      if (!resumed) {
        await _showAlert('Resume failed', 'Could not establish a session');
        return false;
      }
      await _showAlert('Signed in', 'Session established');
      return true;
    } catch (e) {
      await _showAlert('Passkey error', '$e');
      return false;
    }
  }

  Future<void> completePairing() async {
    setState(() {
      isSubmitting = true;
    });
    try {
      final code = pairingCodeController.text.trim().toLowerCase();
      if (code.isEmpty) {
        await _showAlert('Missing info', 'Enter pairing code');
        return;
      }
      if (_sessionId == null) {
        final ok = await _signInWithPasskey();
        if (!ok) return;
      }
      final deviceId = await _ensureDeviceId();
      final uri = _api('/api/pair');
      final body = jsonEncode({'pairing_code': code, 'device_id': deviceId});
      final res = await http
          .post(
            uri,
            headers: {'Content-Type': 'application/json'},
            body: body,
          )
          .timeout(const Duration(seconds: 8));
      if (res.statusCode == 200) {
        final j = jsonDecode(res.body) as Map<String, dynamic>;
        if (j['ok'] == true) {
          await _showAlert('Paired', 'Session: ${j['session_id']}');
          if (!mounted) return;
          Navigator.of(context).pushNamed('/approvals');
        } else {
          await _showAlert('Pairing failed', '${j['error']}');
        }
      } else if (res.statusCode == 410) {
        await _showAlert('Expired', 'Pairing expired. Start again.');
      } else if (res.statusCode == 404) {
        await _showAlert('Not found', 'Pairing not found. Check the code or restart.');
      } else {
        await _showAlert('Error', 'Unexpected error (${res.statusCode})');
      }
    } catch (e) {
      await _showAlert('Network error', '$e');
    } finally {
      setState(() => isSubmitting = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      appBar: AppBar(title: const Text('🔐 Transaction Approver')),
      body: Padding(
        padding: const EdgeInsets.all(16),
        child: ListView(
          children: [
            if (_sessionId != null && _rpcEndpoint != null) ...[
              Text('Session: ${_sessionId}', style: const TextStyle(fontSize: 14)),
              const SizedBox(height: 4),
              Text('RPC: ${_rpcEndpoint}', style: const TextStyle(fontSize: 14)),
              const SizedBox(height: 12),
            ],
            const Text('Enter Pairing Code', style: TextStyle(fontSize: 18, fontWeight: FontWeight.bold)),
            const SizedBox(height: 8),
            TextField(
              controller: pairingCodeController,
              maxLength: 6,
              keyboardType: TextInputType.text,
              textCapitalization: TextCapitalization.none,
              autocorrect: false,
              decoration: const InputDecoration(hintText: 'Enter 6-digit code'),
            ),
            const SizedBox(height: 8),
            OutlinedButton.icon(
              onPressed: () async {
                final code = await Navigator.of(context).push<String>(
                  MaterialPageRoute(builder: (_) => const QrScanPage()),
                );
                if (code != null && code.isNotEmpty && mounted) {
                  pairingCodeController.text = code.toLowerCase();
                  await completePairing();
                }
              },
              icon: const Icon(Icons.qr_code_scanner),
              label: const Text('Scan QR'),
            ),
            const SizedBox(height: 12),
            Row(
              children: [
                Expanded(
                  child: OutlinedButton.icon(
                    onPressed: isBusy ? null : _registerPasskey,
                    icon: const Icon(Icons.key),
                    label: Text(isBusy ? 'Registering…' : 'Register Passkey'),
                  ),
                ),
              ],
            ),
            const SizedBox(height: 8),
            Row(
              children: [
                Expanded(
                  child: OutlinedButton.icon(
                    onPressed: isBusy ? null : () async {
                      setState(() => isBusy = true);
                      try { await _signInWithPasskey(); } finally { if (mounted) setState(() => isBusy = false); }
                    },
                    icon: const Icon(Icons.login),
                    label: Text(isBusy ? 'Signing in…' : 'Sign In (Passkey)'),
                  ),
                ),
              ],
            ),
            const SizedBox(height: 12),
            ElevatedButton(
              onPressed: isSubmitting ? null : completePairing,
              child: Text(isSubmitting ? 'Connecting…' : 'Connect Device'),
            ),
          ],
        ),
      ),
    );
  }
}


