import 'package:flutter/material.dart';
import 'package:flutter_hbb/generated_bridge.dart';
import 'package:flutter_hbb/mobile/pages/connection_page.dart';
import 'package:flutter_hbb/mobile/pages/home_page.dart';
import 'package:flutter_hbb/models/platform_model.dart';
import 'package:flutter_test/flutter_test.dart';

class _LegacyHostBinding extends Fake implements RustdeskImpl {
  @override
  String translate({required String name, required String locale, dynamic hint}) =>
      name;

  @override
  String mainGetAppNameSync({dynamic hint}) => 'RustDesk';

  @override
  bool isIncomingOnly({dynamic hint}) => true;

  @override
  bool isOutgoingOnly({dynamic hint}) => false;
}

class _Context extends Fake implements BuildContext {}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  test('mobile home keeps client navigation with a legacy host-only config', () {
    platformFFI.setBindingForTesting(_LegacyHostBinding());
    final state = HomePageState()..initPages();
    final page = state.build(_Context()) as WillPopScope;
    final scaffold = page.child as Scaffold;
    final navigation = scaffold.bottomNavigationBar as BottomNavigationBar;

    expect(navigation.items.map((item) => item.label), ['Connection', 'Settings']);
    expect(scaffold.body, isA<ConnectionPage>());
  });
}
