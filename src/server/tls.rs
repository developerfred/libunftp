use native_tls::Identity;
use rustls::NoClientAuth;
use std::fs::File;
use std::io::BufReader;
use std::io::Read;
use std::path::Path;
use std::sync::Arc;

/// Creates a native-tls Identity from the specified DER-formatted PKCS #12 archive.
pub fn identity<P: AsRef<Path>, T: Into<String>>(identity_file: P, password: T) -> Identity {
    let mut file = File::open(identity_file).unwrap();
    let mut identity = vec![];
    file.read_to_end(&mut identity).unwrap();
    let pw: String = password.into();
    Identity::from_pkcs12(&identity, &pw).unwrap()
}

// I had to switch to native TLS because of conflicts when trying to use rustls and specifically
// tokio-rustls. Keeping this here for now in case we're switching back
#[allow(unused)]
pub fn new_config<P: AsRef<Path>>(certs_file: P, key_file: P) -> Arc<rustls::ServerConfig> {
    let certs = load_certs(certs_file);
    let privkey = load_private_key(key_file);

    let mut config = rustls::ServerConfig::new(NoClientAuth::new());
    config.key_log = Arc::new(rustls::KeyLogFile::new());
    config.set_single_cert(certs, privkey).expect("Failed to setup TLS certificate chain and key");
    Arc::new(config)
}

// I had to switch to native TLS because of conflicts when trying to use rustls and specifically
// tokio-rustls. Keeping this here for now in case we're switching back
#[allow(unused)]
fn load_certs<P: AsRef<Path>>(filename: P) -> Vec<rustls::Certificate> {
    let certfile = File::open(filename).expect("cannot open certificate file");
    let mut reader = BufReader::new(certfile);
    rustls::internal::pemfile::certs(&mut reader).unwrap()
}

// I had to switch to native TLS because of conflicts when trying to use rustls and specifically
// tokio-rustls. Keeping this here for now in case we're switching back
#[allow(unused)]
fn load_private_key<P: AsRef<Path>>(filename: P) -> rustls::PrivateKey {
    let rsa_keys = {
        let keyfile = File::open(&filename).expect("cannot open private key file");
        let mut reader = BufReader::new(keyfile);
        rustls::internal::pemfile::rsa_private_keys(&mut reader).expect("file contains invalid rsa private key")
    };

    let pkcs8_keys = {
        let keyfile = File::open(&filename).expect("cannot open private key file");
        let mut reader = BufReader::new(keyfile);
        rustls::internal::pemfile::pkcs8_private_keys(&mut reader).expect("file contains invalid pkcs8 private key (encrypted keys not supported)")
    };

    // prefer to load pkcs8 keys
    if !pkcs8_keys.is_empty() {
        pkcs8_keys[0].clone()
    } else {
        assert!(!rsa_keys.is_empty());
        rsa_keys[0].clone()
    }
}
