import 'package:flutter_hbb/common/formatter/id_formatter.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  test('direct endpoint input preserves addresses and does not rewrite IDs', () {
    for (final endpoint in [
      '192.168.1.10:23000',
      '[2001:db8::5]:23000',
      'office-pc.local',
      'office-pc.local:23000',
      '123456789',
      r'123456789/r',
    ]) {
      expect(formatID(endpoint), endpoint);
      expect(trimID(' $endpoint '), endpoint);
    }
  });
}
