import 'package:flutter/material.dart';
import 'package:http/http.dart' as http;
import 'dart:convert';
import 'dart:io' show Platform;
import 'package:mobile_scanner/mobile_scanner.dart';

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
  final TextEditingController deviceIdController = TextEditingController(text: 'device-${DateTime.now().millisecondsSinceEpoch}');
  bool isSubmitting = false;
  

  Future<void> completePairing() async {
    setState(() {
      isSubmitting = true;
      
    });
    try {
      final code = pairingCodeController.text.trim().toLowerCase();
      final deviceId = deviceIdController.text.trim();
      if (code.isEmpty || deviceId.isEmpty) {
        await _showAlert('Missing info', 'Enter pairing code and device id');
        return;
      }
      final host = (Platform.isAndroid ? '127.0.0.1' : '127.0.0.1');
      final uri = Uri.parse('http://$host:3000/api/pair');
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
            const Text('Device ID', style: TextStyle(fontSize: 16, fontWeight: FontWeight.bold)),
            TextField(
              controller: deviceIdController,
              decoration: const InputDecoration(hintText: 'e.g., iPhone-13'),
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
