import 'package:flutter/material.dart';
import 'bootstrap_app.dart';
import 'src/bootstrap_control.dart';

void main() {
  WidgetsFlutterBinding.ensureInitialized();
  if (const bool.fromEnvironment('M0C_DRIVER')) {
    runBootstrapControl();
  } else {
    runApp(const BootstrapApp());
  }
}
