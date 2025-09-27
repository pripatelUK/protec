import 'package:flutter/material.dart';
import 'package:mobile_scanner/mobile_scanner.dart';

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


