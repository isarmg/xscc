import 'dart:async';
import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:flutter_hbb/common.dart';
import 'package:flutter_hbb/models/platform_model.dart';

class ManagedHosts extends StatefulWidget {
  const ManagedHosts({super.key});

  @override
  State<ManagedHosts> createState() => _ManagedHostsState();
}

class _ManagedHostsState extends State<ManagedHosts> {
  Timer? _timer;
  Map<String, dynamic>? _status;
  String? _error;
  bool _polling = false;
  bool _busy = false;

  @override
  void initState() {
    super.initState();
    _refresh();
    _timer = Timer.periodic(const Duration(seconds: 3), (_) => _refresh());
  }

  @override
  void dispose() {
    _timer?.cancel();
    super.dispose();
  }

  Future<dynamic> _request(Map<String, dynamic> request) async {
    final result = jsonDecode(
      await bind.mainManagedRequest(request: jsonEncode(request)),
    ) as Map<String, dynamic>;
    if (result['error'] != null) {
      throw Exception(result['error']);
    }
    return result['value'];
  }

  Future<void> _refresh() async {
    if (_polling || _busy) return;
    _polling = true;
    try {
      final status = await _request({'type': 'status'}) as Map<String, dynamic>;
      if (mounted) {
        setState(() {
          _status = status;
          _error = null;
        });
      }
    } catch (error) {
      if (mounted) {
        setState(() {
          _status = null;
          _error = '$error';
        });
      }
    } finally {
      _polling = false;
    }
  }

  Future<void> _run(Map<String, dynamic> request) async {
    if (_busy) return;
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      final status = await _request(request) as Map<String, dynamic>;
      if (mounted) setState(() => _status = status);
    } catch (error) {
      if (mounted) setState(() => _error = '$error');
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  Future<void> _settings() async {
    final request = await showDialog<Map<String, dynamic>>(
      context: context,
      builder: (_) => _ManagedSettings(status: _status),
    );
    if (mounted && request != null) await _run(request);
  }

  Future<void> _unbind() async {
    final confirmed = await showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        title: const Text('解除实例绑定'),
        content: const Text('解除后当前受管连接将断开。之后可使用授权码重新绑定实例。'),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(context, false),
            child: const Text('取消'),
          ),
          FilledButton(
            onPressed: () => Navigator.pop(context, true),
            child: const Text('解除绑定'),
          ),
        ],
      ),
    );
    if (confirmed == true && mounted) await _run({'type': 'unbind'});
  }

  @override
  Widget build(BuildContext context) {
    final status = _status;
    final configured = status?['configured'] == true;
    final peers = (status?['peers'] as List?) ?? [];
    final role = status?['role'];
    return Padding(
      padding: const EdgeInsets.all(8),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          const Text(
            'XSCC 实例管理',
            style: TextStyle(fontWeight: FontWeight.bold),
          ),
          const SizedBox(height: 6),
          if (_busy) const LinearProgressIndicator(),
          if (_error != null)
            Text(
              _error!,
              style: TextStyle(color: Theme.of(context).colorScheme.error),
            ),
          Text(
            !configured
                ? '尚未绑定实例'
                : '${role == 'controller'
                      ? '控制端'
                      : role == 'controlled'
                      ? '被控端'
                      : '实例'} · ${status?['phase'] ?? 'offline'}',
          ),
          if (status?['binding'] != null)
            SelectableText(
              '${status!['binding']['device_id']}',
              style: const TextStyle(fontSize: 11),
            ),
          OutlinedButton(
            onPressed: _busy ? null : _settings,
            child: Text(configured ? '实例管理设置' : '绑定实例'),
          ),
          if (configured)
            Wrap(
              spacing: 4,
              children: [
                TextButton(
                  onPressed: _busy ? null : () => _run({'type': 'resume'}),
                  child: const Text('重新连接'),
                ),
                TextButton(
                  onPressed: _busy ? null : _unbind,
                  child: const Text('解绑'),
                ),
              ],
            ),
          if (configured) Text(role == 'controlled' ? '允许的控制端' : '允许连接的被控端'),
          for (final peer in peers)
            ListTile(
              dense: true,
              contentPadding: EdgeInsets.zero,
              title: Text('${peer['name']}'),
              subtitle: Text(peer['online'] == true ? '在线' : '离线'),
              trailing: role == 'controller'
                  ? IconButton(
                      tooltip: '连接',
                      icon: const Icon(Icons.computer),
                      onPressed:
                          !_busy &&
                              peer['online'] == true &&
                              (peer['addresses'] as List).isNotEmpty
                          ? () => connect(
                              context,
                              "managed:${peer['binding']['device_id']}",
                            )
                          : null,
                    )
                  : null,
            ),
          if (configured && peers.isEmpty) const Text('暂无获准主机；请在服务端网页设置授权关系。'),
        ],
      ),
    );
  }
}

class _ManagedSettings extends StatefulWidget {
  final Map<String, dynamic>? status;
  const _ManagedSettings({required this.status});

  @override
  State<_ManagedSettings> createState() => _ManagedSettingsState();
}

class _ManagedSettingsState extends State<_ManagedSettings> {
  late final TextEditingController _server;
  final _code = TextEditingController();
  late final TextEditingController _addresses;
  String? _error;

  bool get _configured => widget.status?['configured'] == true;

  @override
  void initState() {
    super.initState();
    _server = TextEditingController(
      text: widget.status?['server'] as String? ?? '',
    );
    _addresses = TextEditingController(
      text: ((widget.status?['addresses'] as List?) ?? [])
          .map((a) {
            final host = a['host'].toString();
            return "${host.contains(':') ? '[$host]' : host}:${a['port']}";
          })
          .join('\n'),
    );
  }

  @override
  void dispose() {
    _server.dispose();
    _code.dispose();
    _addresses.dispose();
    super.dispose();
  }

  void _save() {
    try {
      final addresses = _addresses.text
          .split(RegExp(r'[\r\n]+'))
          .where((s) => s.trim().isNotEmpty)
          .map((s) {
            final uri = Uri.tryParse('tcp://${s.trim()}');
            if (uri == null ||
                uri.host.isEmpty ||
                uri.userInfo.isNotEmpty ||
                uri.path.isNotEmpty ||
                uri.hasQuery ||
                uri.hasFragment) {
              throw const FormatException('请输入 IP 或域名，可附加端口；IPv6 使用 [地址]:端口');
            }
            var host = uri.host.toLowerCase();
            if (host.startsWith('[') && host.endsWith(']')) {
              host = host.substring(1, host.length - 1);
            }
            return <String, dynamic>{
              'host': host,
              'port': uri.hasPort ? uri.port : 21118,
            };
          })
          .toList();
      if (addresses.length > 4) throw const FormatException('最多填写 4 个连接地址');
      if (!_configured && _code.text.trim().isEmpty) {
        throw const FormatException('请输入实例授权码');
      }
      Navigator.pop(
        context,
        _code.text.trim().isNotEmpty || !_configured
            ? <String, dynamic>{
                'type': 'enroll',
                'server': _server.text.trim(),
                'authorization_code': _code.text.trim(),
                'addresses': addresses,
              }
            : <String, dynamic>{
                'type': 'set_addresses',
                'addresses': addresses,
              },
      );
    } catch (error) {
      setState(() => _error = '$error');
    }
  }

  @override
  Widget build(BuildContext context) => AlertDialog(
    title: Text(_configured ? '实例管理设置' : '绑定服务端实例'),
    content: SizedBox(
      width: 460,
      child: SingleChildScrollView(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            TextField(
              controller: _server,
              enabled: !_configured,
              decoration: const InputDecoration(
                labelText: '服务端地址',
                hintText: 'https://server.example.com',
              ),
            ),
            if (!_configured ||
                widget.status?['binding'] == null ||
                widget.status?['phase'] == 'revoked')
              TextField(
                controller: _code,
                obscureText: true,
                autocorrect: false,
                enableSuggestions: false,
                decoration: const InputDecoration(labelText: '实例授权码'),
              ),
            const SizedBox(height: 12),
            if (isDesktop)
              TextField(
                controller: _addresses,
                minLines: 2,
                maxLines: 4,
                decoration: const InputDecoration(
                  labelText: '被控端连接地址',
                  hintText: 'host.example.com:21118',
                  helperText: '每行一个，最多 4 个。留空则自动上报本机地址。',
                ),
              ),
            const SizedBox(height: 12),
            Text(
              isDesktop
                  ? '实例角色和允许连接的主机由服务端网页设置。画面、编码和硬件选项在控制端会话内选择。'
                  : '请绑定控制端实例。允许连接的被控主机在服务端网页设置。',
            ),
            if (_error != null)
              Text(
                _error!,
                style: TextStyle(color: Theme.of(context).colorScheme.error),
              ),
          ],
        ),
      ),
    ),
    actions: [
      TextButton(
        onPressed: () => Navigator.pop(context),
        child: const Text('取消'),
      ),
      FilledButton(onPressed: _save, child: Text(_configured ? '保存' : '绑定')),
    ],
  );
}
