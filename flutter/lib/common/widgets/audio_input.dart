import 'package:flutter/material.dart';
import 'package:flutter_hbb/common.dart';
import 'package:flutter_hbb/models/platform_model.dart';

const _kSystemSound = 'System Sound';

typedef AudioINputSetDevice = void Function(String device);
typedef AudioInputBuilder = Widget Function(
    List<String> devices, String currentDevice, AudioINputSetDevice setDevice);

class AudioInput extends StatelessWidget {
  final AudioInputBuilder builder;
  final bool isCm;

  const AudioInput(
      {Key? key,
      required this.builder,
      required this.isCm})
      : super(key: key);

  static String getDefault() {
    if (bind.mainAudioSupportLoopback()) return translate(_kSystemSound);
    return '';
  }

  static Future<String> getAudioInput(bool isCm) {
    return bind.mainGetOption(key: 'audio-input');
  }

  static Future<String> getValue(bool isCm) async {
    String device = await getAudioInput(isCm);
    if (device.isNotEmpty) {
      return device;
    } else {
      return getDefault();
    }
  }

  static Future<void> setDevice(
      String device, bool isCm) async {
    if (device == getDefault()) device = '';
    await bind.mainSetOption(key: 'audio-input', value: device);
  }

  static Future<Map<String, Object>> getDevicesInfo(
      bool isCm) async {
    List<String> devices = (await bind.mainGetSoundInputs()).toList();
    if (bind.mainAudioSupportLoopback()) {
      devices.insert(0, translate(_kSystemSound));
    }
    String current = await getValue(isCm);
    return {'devices': devices, 'current': current};
  }

  @override
  Widget build(BuildContext context) {
    return futureBuilder(
      future: getDevicesInfo(isCm),
      hasData: (data) {
        String currentDevice = data['current'];
        List<String> devices = data['devices'] as List<String>;
        if (devices.isEmpty) {
          return const Offstage();
        }
        return builder(devices, currentDevice, (devices) {
          setDevice(devices, isCm);
        });
      },
    );
  }
}
