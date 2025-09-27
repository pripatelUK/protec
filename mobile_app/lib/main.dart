import 'package:flutter/material.dart';
import 'screens/auth_screen.dart';
import 'screens/pairing_screen.dart';
import 'screens/approval_screen.dart';

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
      home: const AuthScreen(),
      routes: {
        '/pair': (_) => const PairingScreen(),
        '/approvals': (_) => const ApprovalScreen(),
      },
    );
  }
}
// Screens moved to lib/screens/
