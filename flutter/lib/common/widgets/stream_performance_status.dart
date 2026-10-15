import 'package:flutter/material.dart';

class StreamPerformanceStatus extends StatelessWidget {
  const StreamPerformanceStatus({
    super.key,
    required this.fps,
    required this.codec,
    required this.translate,
  });

  final String? fps;
  final String? codec;
  final String Function(String) translate;

  @override
  Widget build(BuildContext context) {
    final style = Theme.of(context).textTheme.bodySmall;
    return SizedBox(
      width: 280,
      child: Padding(
        padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 8),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text(translate('Latest received'), style: style),
            Text('${translate('Actual frame rate')}: ${fps ?? '-'} FPS'),
            Text('${translate('Actual codec')}: ${codec ?? '-'}'),
            const SizedBox(height: 8),
            Text(
              translate(
                  'Preferences are saved for this peer. Encoding is shared with other viewers.'),
              style: style,
            ),
          ],
        ),
      ),
    );
  }
}
