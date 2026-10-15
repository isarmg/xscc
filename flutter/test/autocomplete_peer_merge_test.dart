import 'package:flutter_hbb/common/widgets/autocomplete.dart';
import 'package:flutter_hbb/models/peer_model.dart';
import 'package:flutter_test/flutter_test.dart';

Peer _peer({
  required String id,
  String alias = '',
  String username = '',
  String hostname = '',
  bool online = false,
}) {
  final peer = Peer(
    id: id,
    username: username,
    hostname: hostname,
    alias: alias,
    platform: '',
  );
  peer.online = online;
  return peer;
}

void main() {
  test('merged autocomplete peers retain LAN identity and recent online state', () {
    final peers = mergeAutocompletePeers(
      lanPeers: [_peer(id: '192.168.1.10:21118', alias: 'Office PC', username: 'lan-user')],
      recentPeers: [_peer(id: '192.168.1.10:21118', online: true)],
    );
    expect(peers, hasLength(1));
    expect(peers.single.alias, 'Office PC');
    expect(peers.single.username, 'lan-user');
    expect(peers.single.online, isTrue);
  });

}
