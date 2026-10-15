import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:flutter_hbb/models/input_modifier_utils.dart';

void main() {
  group('shouldReleaseStaleMobileShift', () {
    test('releases one-shot mobile shift after a text key', () {
      expect(
        shouldReleaseStaleMobileShift(
          isMobile: true,
          cachedShiftPressed: true,
          actualShiftPressed: false,
          logicalKey: LogicalKeyboardKey.keyD,
          hasTrackedShiftKeyDown: true,
        ),
        isTrue,
      );
    });

    test('does not release when shift is still physically pressed', () {
      expect(
        shouldReleaseStaleMobileShift(
          isMobile: true,
          cachedShiftPressed: true,
          actualShiftPressed: true,
          logicalKey: LogicalKeyboardKey.keyD,
          hasTrackedShiftKeyDown: true,
        ),
        isFalse,
      );
    });
  });

}
