import 'dart:convert';
import 'dart:io' show Platform;
import 'package:flutter/material.dart';
import 'package:http/http.dart' as http;
import 'package:passkeys/authenticator.dart';
import 'package:passkeys/types.dart';
import 'package:shared_preferences/shared_preferences.dart';

class AuthScreen extends StatefulWidget {
  const AuthScreen({super.key});

  @override
  State<AuthScreen> createState() => _AuthScreenState();
}

class _AuthScreenState extends State<AuthScreen> {
  final TextEditingController _emailController = TextEditingController();
  final PasskeyAuthenticator _auth = PasskeyAuthenticator(debugMode: true);

  static const _prefsEmailKey = 'user_email';
  static const _prefsSessionIdKey = 'session_id';
  static const _prefsRpcEndpointKey = 'rpc_endpoint';

  bool _busy = false;

  String get _apiBaseHost => (Platform.isAndroid ? '127.0.0.1' : '127.0.0.1');
  Uri _api(String path) => Uri.parse('http://$_apiBaseHost:3000$path');

  @override
  void initState() {
    super.initState();
    _loadEmail();
  }

  Future<void> _loadEmail() async {
    final prefs = await SharedPreferences.getInstance();
    final saved = prefs.getString(_prefsEmailKey);
    if (saved != null && saved.isNotEmpty) {
      _emailController.text = saved;
    }
  }

  bool _isValidEmail(String s) {
    return RegExp(r'^[^@\s]+@[^@\s]+\.[^@\s]+$').hasMatch(s);
  }

  Future<void> _storeEmail(String email) async {
    final prefs = await SharedPreferences.getInstance();
    await prefs.setString(_prefsEmailKey, email);
  }

  Future<void> _persistSession(String sessionId, String rpcEndpoint) async {
    final prefs = await SharedPreferences.getInstance();
    await prefs.setString(_prefsSessionIdKey, sessionId);
    await prefs.setString(_prefsRpcEndpointKey, rpcEndpoint);
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
        await _persistSession(j['session_id'] as String, j['rpc_endpoint'] as String);
        return true;
      }
      return false;
    } catch (_) {
      return false;
    }
  }

  Future<void> _register() async {
    final email = _emailController.text.trim().toLowerCase();
    if (!_isValidEmail(email)) {
      await _alert('Invalid email', 'Enter a valid email address');
      return;
    }
    setState(() => _busy = true);
    try {
      final startRes = await http
          .post(
            _api('/api/passkeys/register/start'),
            headers: {'Content-Type': 'application/json'},
            body: jsonEncode({'email': email, 'display_name': email}),
          )
          .timeout(const Duration(seconds: 10));
      if (startRes.statusCode != 200) {
        await _alert('Error', 'Register/start failed (${startRes.statusCode})');
        return;
      }
      final options = jsonDecode(startRes.body) as Map<String, dynamic>;
      final pk = (options['publicKey'] as Map<String, dynamic>?) ?? options;
      final rp = pk['rp'] as Map<String, dynamic>?;
      final userMap = pk['user'] as Map<String, dynamic>?;
      final challenge = pk['challenge'] as String;
      debugPrint('server.register publicKey.rp.id=${rp?['id']} - ${rp?['name']} challenge.len=${challenge.length}');

      final userId = (userMap?['id'] as String?) ?? base64Url.encode(utf8.encode(email));
      final userName = (userMap?['name'] as String?) ?? email;
      final userDisplay = (userMap?['displayName'] as String?) ?? email;
      final req = RegisterRequestType(
        challenge: challenge,
        relyingParty: RelyingPartyType(
          name: (rp?['name'] as String?) ?? 'Protec',
          id: (rp?['id'] as String?) ?? _apiBaseHost,
        ),
        user: UserType(
          displayName: userDisplay,
          name: userName,
          id: userId,
        ),
        excludeCredentials: const [],
        // ES256 to satisfy passkeys_android expectations
        pubKeyCredParams: [
          PubKeyCredParamType(type: 'public-key', alg: -7),
        ],
        timeout: 60000,
        attestation: 'none',
      );
      debugPrint('passkeys.register -> rp.id=${req.relyingParty.id} rp.name=${req.relyingParty.name} user.name=$email user.id.len=${userId.length} challenge.len=${req.challenge.length}');
      final reg = await _auth.register(req);

      final finishRes = await http
          .post(
            _api('/api/passkeys/register/finish'),
            headers: {'Content-Type': 'application/json'},
            body: jsonEncode({
              'email': email,
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
        await _storeEmail(email);
        await _alert('Account created', 'Passkey registered');
      } else {
        await _alert('Error', 'Register/finish failed (${finishRes.statusCode})');
      }
    } catch (e) {
      await _alert('Passkey error', '$e');
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  Future<void> _signIn() async {
    setState(() => _busy = true);
    try {
      final startRes = await http
          .post(
            _api('/api/passkeys/assert/start'),
            headers: {'Content-Type': 'application/json'},
            body: jsonEncode({}),
          )
          .timeout(const Duration(seconds: 10));
      if (startRes.statusCode != 200) {
        await _alert('Error', 'Assert/start failed (${startRes.statusCode})');
        return;
      }
      final requestOptions = jsonDecode(startRes.body) as Map<String, dynamic>;
      final pk = (requestOptions['publicKey'] as Map<String, dynamic>?) ?? requestOptions;
      final rpId = pk['rpId'] as String;
      final challenge = pk['challenge'] as String;
      debugPrint('server.assert publicKey.rpId=$rpId challenge.len=${challenge.length} allow.len=${(pk['allowCredentials'] as List<dynamic>? ?? []).length}');
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

      debugPrint('passkeys.assert -> rpId=$rpId challenge.len=${challenge.length} allow=${allow.length}');
      final assertion = await _auth.authenticate(authReq);

      final finishRes = await http
          .post(
            _api('/api/passkeys/assert/finish'),
            headers: {'Content-Type': 'application/json'},
            body: jsonEncode({
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
        await _alert('Error', 'Assert/finish failed (${finishRes.statusCode})');
        return;
      }
      final j = jsonDecode(finishRes.body) as Map<String, dynamic>;
      final token = j['assertion_token'] as String?;
      if (token == null) {
        await _alert('Error', 'No token returned');
        return;
      }
      final ok = await _resumeWithToken(token);
      if (!ok) {
        await _alert('Resume failed', 'Could not establish a session');
        return;
      }
      if (!mounted) return;
      Navigator.of(context).pushReplacementNamed('/pair');
    } catch (e) {
      await _alert('Passkey error', '$e');
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  Future<void> _alert(String title, String message) async {
    if (!mounted) return;
    await showDialog<void>(
      context: context,
      builder: (context) => AlertDialog(
        title: Text(title),
        content: Text(message),
        actions: [TextButton(onPressed: () => Navigator.of(context).pop(), child: const Text('OK'))],
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Scaffold(
      body: SafeArea(
        child: Padding(
          padding: const EdgeInsets.symmetric(horizontal: 24, vertical: 16),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.center,
            children: [
              const SizedBox(height: 32),
              Icon(Icons.storage_rounded, size: 72, color: theme.colorScheme.onSurfaceVariant),
              const SizedBox(height: 24),
              const Text(
                'Protect your\nTransactions',
                textAlign: TextAlign.center,
                style: TextStyle(fontSize: 36, fontWeight: FontWeight.w700),
              ),
              const SizedBox(height: 16),
              Text(
                'Protec is a seamless, plug & play\nway to prevent phishing attacks.',
                textAlign: TextAlign.center,
                style: TextStyle(
                  fontSize: 16,
                  color: theme.colorScheme.onSurfaceVariant,
                  height: 1.35,
                ),
              ),
              const SizedBox(height: 24),
              TextField(
                controller: _emailController,
                keyboardType: TextInputType.emailAddress,
                autofillHints: const [AutofillHints.email],
                decoration: const InputDecoration(
                  hintText: 'example@ithaca.xyz',
                  border: OutlineInputBorder(borderRadius: BorderRadius.all(Radius.circular(28))),
                ),
              ),
              const SizedBox(height: 16),
              SizedBox(
                width: double.infinity,
                child: OutlinedButton(
                  onPressed: _busy ? null : _register,
                  style: OutlinedButton.styleFrom(padding: const EdgeInsets.symmetric(vertical: 16)),
                  child: Text(_busy ? 'Please wait…' : 'Create account'),
                ),
              ),
              Padding(
                padding: const EdgeInsets.symmetric(vertical: 12),
                child: Row(
                  children: const [
                    Expanded(child: Divider()),
                    Padding(padding: EdgeInsets.symmetric(horizontal: 8), child: Text('or')),
                    Expanded(child: Divider()),
                  ],
                ),
              ),
              SizedBox(
                width: double.infinity,
                child: FilledButton.icon(
                  onPressed: _busy ? null : _signIn,
                  icon: const Icon(Icons.face_6_rounded),
                  label: Text(_busy ? 'Signing in…' : 'Sign in'),
                  style: FilledButton.styleFrom(padding: const EdgeInsets.symmetric(vertical: 16)),
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }
}


