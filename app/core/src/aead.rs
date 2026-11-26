pub(crate) use yoip::protocol::aead::open;

pub(crate) fn seal(key: &[u8; 32], aad: &[u8], plain: &[u8]) -> Vec<u8> {
    yoip::protocol::aead::seal(key, aad, plain, crate::identity::random())
}
