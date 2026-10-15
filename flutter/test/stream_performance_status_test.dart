import 'package:flutter/material.dart';
import 'package:flutter_hbb/common/widgets/stream_performance_status.dart';
import 'package:flutter_test/flutter_test.dart';

Widget status({String? fps, String? codec}) => MaterialApp(
      home: Scaffold(
        body: StreamPerformanceStatus(
          fps: fps,
          codec: codec,
          translate: (key) => key,
        ),
      ),
    );

void main() {
  testWidgets('does not invent results before receiving statistics',
      (tester) async {
    await tester.pumpWidget(status());
    expect(find.text('Latest received'), findsOneWidget);
    expect(find.text('Actual frame rate: - FPS'), findsOneWidget);
    expect(find.text('Actual codec: -'), findsOneWidget);
    expect(
      find.text(
          'Preferences are saved for this peer. Encoding is shared with other viewers.'),
      findsOneWidget,
    );
  });

  testWidgets('shows updated received values independently of target choices',
      (tester) async {
    await tester.pumpWidget(status(fps: '24', codec: 'H264'));
    expect(find.text('Actual frame rate: 24 FPS'), findsOneWidget);
    expect(find.text('Actual codec: H264'), findsOneWidget);

    await tester.pumpWidget(status(fps: '57', codec: 'H265'));
    expect(find.text('Actual frame rate: 57 FPS'), findsOneWidget);
    expect(find.text('Actual codec: H265'), findsOneWidget);
    expect(find.text('Actual frame rate: 24 FPS'), findsNothing);
  });
}
