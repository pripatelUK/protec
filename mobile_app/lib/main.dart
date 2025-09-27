import 'package:flutter/material.dart';

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

class PairingScreen extends StatelessWidget {
  const PairingScreen({super.key});

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      appBar: AppBar(title: const Text('🔐 Transaction Approver')),
      body: Padding(
        padding: const EdgeInsets.all(16),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            const Text('Enter Pairing Code', style: TextStyle(fontSize: 18, fontWeight: FontWeight.bold)),
            const SizedBox(height: 8),
            const TextField(maxLength: 6, keyboardType: TextInputType.number, decoration: InputDecoration(hintText: 'Enter 6-digit code')),
            const SizedBox(height: 12),
            ElevatedButton(
              onPressed: () {
                Navigator.of(context).pushNamed('/approvals');
              },
              child: const Text('Connect Device'),
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
