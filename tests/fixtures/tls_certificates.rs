// Shared OpenSSL certificate fixture for actual HTTPS and WSS tests.
use std::{path::Path, process::Command as ProcessCommand};

fn openssl(path: &Path, args: &[&str]) {
    let output = ProcessCommand::new("openssl")
        .current_dir(path)
        .args(args)
        .output()
        .expect("openssl is a required test dependency");
    assert!(
        output.status.success(),
        "test certificate generation failed: openssl {args:?} exited {}; stderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr[output.stderr.len().saturating_sub(8192)..])
    );
}

pub fn certificates(path: &Path) {
    // The system LibreSSL on macOS lacks newer OpenSSL CLI options such as
    // -addext and x509 -copy_extensions. Explicit extensions keep the fixture's
    // CA and HTTPS certificate properties independent of the host defaults.
    std::fs::write(
        path.join("openssl-fixture.cnf"),
        "[req]\nprompt = no\ndistinguished_name = fixture_subject\n\
         [fixture_subject]\nCN = localhost\n\
         [fixture_ca]\nbasicConstraints = critical,CA:TRUE\n\
         [fixture_sunshine]\nbasicConstraints = critical,CA:FALSE\n\
         [fixture_server]\nsubjectAltName = IP:127.0.0.1\n\
         basicConstraints = critical,CA:FALSE\nextendedKeyUsage = serverAuth\n",
    )
    .unwrap();
    openssl(
        path,
        &[
            "req",
            "-x509",
            "-sha256",
            "-newkey",
            "rsa:2048",
            "-nodes",
            "-keyout",
            "ca.key",
            "-out",
            "ca.pem",
            "-days",
            "1",
            "-subj",
            "/CN=Client Test CA",
            "-config",
            "openssl-fixture.cnf",
            "-extensions",
            "fixture_ca",
        ],
    );
    // Mirrors Sunshine's built-in certificate: self-signed CN, without a loopback SAN.
    // An explicit leaf extension forces v3 even when the host adds no default extensions.
    openssl(
        path,
        &[
            "req",
            "-x509",
            "-sha256",
            "-newkey",
            "rsa:2048",
            "-nodes",
            "-keyout",
            "sunshine.key",
            "-out",
            "sunshine.pem",
            "-days",
            "1",
            "-subj",
            "/CN=Sunshine Gamestream Host",
            "-config",
            "openssl-fixture.cnf",
            "-extensions",
            "fixture_sunshine",
        ],
    );
    openssl(
        path,
        &[
            "req",
            "-new",
            "-sha256",
            "-newkey",
            "rsa:2048",
            "-nodes",
            "-keyout",
            "server.key",
            "-out",
            "server.csr",
            "-subj",
            "/CN=localhost",
            "-config",
            "openssl-fixture.cnf",
        ],
    );
    openssl(
        path,
        &[
            "x509",
            "-req",
            "-sha256",
            "-in",
            "server.csr",
            "-CA",
            "ca.pem",
            "-CAkey",
            "ca.key",
            "-CAcreateserial",
            "-out",
            "server.pem",
            "-days",
            "1",
            "-extfile",
            "openssl-fixture.cnf",
            "-extensions",
            "fixture_server",
        ],
    );
    openssl(
        path,
        &[
            "req",
            "-x509",
            "-sha256",
            "-newkey",
            "rsa:2048",
            "-nodes",
            "-keyout",
            "wrong.key",
            "-out",
            "wrong.pem",
            "-days",
            "1",
            "-subj",
            "/CN=Untrusted Test CA",
            "-config",
            "openssl-fixture.cnf",
            "-extensions",
            "fixture_ca",
        ],
    );
}
