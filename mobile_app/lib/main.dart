import 'package:flutter/material.dart';
import 'package:http/http.dart' as http;
import 'dart:convert';
import 'dart:io' show Platform;
import 'package:mobile_scanner/mobile_scanner.dart';
import 'package:shared_preferences/shared_preferences.dart';
import 'package:passkeys/authenticator.dart';
import 'package:passkeys/types.dart';

void main() {
  runApp(const App());
}

class App extends StatelessWidget {
  const App({super.key});

  @override
  Widget build(BuildContext context) {
    return MaterialApp(
      title: 'Protec Approvals',
      theme: ThemeData(
        colorScheme: ColorScheme.fromSeed(seedColor: Colors.indigo),
        useMaterial3: true,
      ),
      home: const PairingScreen(),
      routes: {
        '/approvals': (_) => const ApprovalScreen(),
      },
    );
  }
}

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
  final PasskeyAuthenticator _auth = PasskeyAuthenticator(debugMode: true);

  String get _apiBaseHost => (Platform.isAndroid ? '127.0.0.1' : '127.0.0.1');
  Uri _api(String path) => Uri.parse('http://$_apiBaseHost:3000$path');

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

  Future<void> _registerPasskey() async {
    if (isBusy) return;
    final code = pairingCodeController.text.trim().toLowerCase();
    if (code.isEmpty) {
      await _showAlert('Missing info', 'Enter pairing code first');
      return;
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
      // Ensure user signs in with a passkey (replaces manual device_id entry)
      final ok = await _signInWithPasskey();
      if (!ok) return;

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

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      appBar: AppBar(title: const Text('🔐 Transaction Approver')),
      body: Padding(
        padding: const EdgeInsets.all(16),
        child: ListView(
          children: [
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

class ApprovalScreen extends StatelessWidget {
  const ApprovalScreen({super.key});

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      appBar: AppBar(title: const Text('Pending Approvals')),
      body: ListView(
        padding: const EdgeInsets.all(16),
        children: const [
          Card(
            child: ListTile(
              title: Text('🔄 Transfer 100 USDC'),
              subtitle: Text('To: 0x742d...3Ba2 (Uniswap V3)'),
              trailing: Text('⚠️ Medium'),
            ),
          ),
        ],
      ),
    );
  }
}

class QrScanPage extends StatefulWidget {
  const QrScanPage({super.key});

  @override
  State<QrScanPage> createState() => _QrScanPageState();
}

class _QrScanPageState extends State<QrScanPage> {
  bool _handled = false;

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      appBar: AppBar(title: const Text('Scan Pairing QR')),
      body: MobileScanner(
        onDetect: (capture) {
          if (_handled) return;
          final codes = capture.barcodes;
          if (codes.isEmpty) return;
          final raw = codes.first.rawValue ?? '';
          if (raw.isEmpty) return;
          _handled = true;
          // Expected format: PAIR:<code>
          final code = raw.startsWith('PAIR:') ? raw.substring(5) : raw;
          Navigator.of(context).pop(code);
        },
      ),
    );
  }
}
