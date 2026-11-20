//! Proof of possession for a linked device using a MeshCore backbone service.

use crate::person::{CERT_LEN, DeviceCert, DeviceList, PersonError};
use crate::{Identity, PublicIdentity, SIGNATURE_LEN, dht_exchange_digest};
use ed25519_dalek::{Signature, Signer, VerifyingKey};
use maarse_core::Address;

const CONTEXT: &[u8] = b"maarse/meshcore-access/v1";

/// The request includes the operation and exact arguments. The server challenge
/// must be generated for this service, bound to its connection and consumed once.
pub fn sign_request(
    device: &Identity,
    challenge: &[u8; 32],
    request: &[u8],
    person: &Address,
) -> [u8; SIGNATURE_LEN] {
    device
        .signing_key()
        .sign(&dht_exchange_digest(
            CONTEXT,
            challenge,
            request,
            person.as_bytes(),
        ))
        .to_bytes()
}

pub struct AuthorizedDevice {
    pub person: Address,
    pub device: Address,
    pub list: DeviceList,
}

/// The caller must reconcile the returned list with the latest signed list in
/// durable storage and check the current administrator grant before acting.
pub fn verify_request(
    device: &PublicIdentity,
    certificate: &[u8; CERT_LEN],
    device_list: &[u8],
    challenge: &[u8; 32],
    request: &[u8],
    signature: &[u8; SIGNATURE_LEN],
) -> Result<AuthorizedDevice, PersonError> {
    let address = device.address();
    let certificate = DeviceCert::verify(&address, certificate)?;
    let person = certificate.person();
    let key = VerifyingKey::from_bytes(&device.sign).map_err(|_| PersonError::Malformed)?;
    key.verify_strict(
        &dht_exchange_digest(CONTEXT, challenge, request, person.as_bytes()),
        &Signature::from_bytes(signature),
    )
    .map_err(|_| PersonError::BadSignature)?;
    let list = DeviceList::verify(&certificate.master, device_list)?;
    if list.is_revoked(&address) || !list.active().any(|active| active == address) {
        return Err(PersonError::WrongDevice);
    }
    Ok(AuthorizedDevice {
        person,
        device: address,
        list,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::person::DeviceEntry;
    use rand_core::OsRng;

    #[test]
    fn linked_device_proof_binds_the_operation_and_challenge() {
        let master = Identity::generate(&mut OsRng);
        let device = Identity::generate(&mut OsRng);
        let address = device.address();
        let cert = DeviceCert::issue(&master, &address, 1).to_bytes();
        let list = DeviceList::issue(
            &master,
            1,
            &[DeviceEntry {
                addr: address,
                revoked: false,
            }],
        )
        .unwrap();
        let challenge = [1; 32];
        let proof = sign_request(&device, &challenge, b"ensure", &master.address());
        let verified = verify_request(
            &device.public(),
            &cert,
            &list.to_bytes(),
            &challenge,
            b"ensure",
            &proof,
        )
        .unwrap();
        assert_eq!(verified.person, master.address());
        assert_eq!(verified.device, address);
        assert!(
            verify_request(
                &device.public(),
                &cert,
                &list.to_bytes(),
                &challenge,
                b"send",
                &proof
            )
            .is_err()
        );
        assert!(
            verify_request(
                &device.public(),
                &cert,
                &list.to_bytes(),
                &[2; 32],
                b"ensure",
                &proof
            )
            .is_err()
        );
        let revoked = DeviceList::issue(
            &master,
            2,
            &[DeviceEntry {
                addr: address,
                revoked: true,
            }],
        )
        .unwrap();
        assert!(
            verify_request(
                &device.public(),
                &cert,
                &revoked.to_bytes(),
                &challenge,
                b"ensure",
                &proof
            )
            .is_err()
        );
    }
}
