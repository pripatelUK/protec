import 'package:flutter/material.dart';

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


