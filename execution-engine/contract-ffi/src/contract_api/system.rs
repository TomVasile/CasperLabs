use alloc::vec::Vec;
use core::fmt::Debug;
use core::u8;

use super::error::Error;
use super::runtime::get_key;
use super::runtime::revert;
use super::{alloc_bytes, to_ptr, ContractRef, TURef};
use crate::bytesrepr::deserialize;
use crate::ext_ffi;
use crate::key::Key;
use crate::value::account::{PublicKey, PurseId, PURSE_ID_SIZE_SERIALIZED};
use crate::value::U512;

pub type TransferResult = Result<TransferredTo, Error>;

pub const MINT_NAME: &str = "mint";
pub const POS_NAME: &str = "pos";

fn get_system_contract(name: &str) -> ContractRef {
    let key = get_key(name).unwrap_or_else(|| revert(Error::GetURef));

    if let Key::URef(uref) = key {
        let reference = TURef::from_uref(uref).unwrap_or_else(|_| revert(Error::NoAccessRights));
        ContractRef::URef(reference)
    } else {
        revert(Error::UnexpectedKeyVariant)
    }
}

/// Returns a read-only pointer to the Mint Contract.  Any failure will trigger `revert()` with a
/// `contract_api::Error`.
pub fn get_mint() -> ContractRef {
    get_system_contract(MINT_NAME)
}

/// Returns a read-only pointer to the Proof of Stake Contract.  Any failure will trigger `revert()`
/// with a `contract_api::Error`.
pub fn get_proof_of_stake() -> ContractRef {
    get_system_contract(POS_NAME)
}

pub fn create_purse() -> PurseId {
    let purse_id_ptr = alloc_bytes(PURSE_ID_SIZE_SERIALIZED);
    unsafe {
        let ret = ext_ffi::create_purse(purse_id_ptr, PURSE_ID_SIZE_SERIALIZED);
        if ret == 0 {
            let bytes = Vec::from_raw_parts(
                purse_id_ptr,
                PURSE_ID_SIZE_SERIALIZED,
                PURSE_ID_SIZE_SERIALIZED,
            );
            deserialize(&bytes).unwrap()
        } else {
            panic!("could not create purse_id")
        }
    }
}

/// Gets the balance of a given purse
pub fn get_balance(purse_id: PurseId) -> Option<U512> {
    let (purse_id_ptr, purse_id_size, _bytes) = to_ptr(&purse_id);

    let balance_bytes: Vec<u8> = unsafe {
        let value_size = ext_ffi::get_balance(purse_id_ptr, purse_id_size) as usize;
        if value_size == 0 {
            return None;
        }
        let dest_ptr = alloc_bytes(value_size);
        ext_ffi::get_read(dest_ptr);
        Vec::from_raw_parts(dest_ptr, value_size, value_size)
    };

    let balance: U512 = deserialize(&balance_bytes).unwrap_or_else(|_| revert(Error::Deserialize));

    Some(balance)
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
#[repr(i32)]
pub enum TransferredTo {
    ExistingAccount = 0,
    NewAccount = 1,
}

impl TransferredTo {
    fn result_from(value: i32) -> TransferResult {
        match value {
            x if x == TransferredTo::ExistingAccount as i32 => Ok(TransferredTo::ExistingAccount),
            x if x == TransferredTo::NewAccount as i32 => Ok(TransferredTo::NewAccount),
            _ => Err(Error::Transfer),
        }
    }

    pub fn i32_from(result: TransferResult) -> i32 {
        match result {
            Ok(transferred_to) => transferred_to as i32,
            Err(_) => 2,
        }
    }
}

/// Transfers `amount` of motes from default purse of the account to `target`
/// account. If `target` does not exist it will create it.
pub fn transfer_to_account(target: PublicKey, amount: U512) -> TransferResult {
    let (target_ptr, target_size, _bytes) = to_ptr(&target);
    let (amount_ptr, amount_size, _bytes) = to_ptr(&amount);
    let return_code =
        unsafe { ext_ffi::transfer_to_account(target_ptr, target_size, amount_ptr, amount_size) };
    TransferredTo::result_from(return_code)
}

/// Transfers `amount` of motes from `source` purse to `target` account.
/// If `target` does not exist it will create it.
pub fn transfer_from_purse_to_account(
    source: PurseId,
    target: PublicKey,
    amount: U512,
) -> TransferResult {
    let (source_ptr, source_size, _bytes) = to_ptr(&source);
    let (target_ptr, target_size, _bytes) = to_ptr(&target);
    let (amount_ptr, amount_size, _bytes) = to_ptr(&amount);
    let return_code = unsafe {
        ext_ffi::transfer_from_purse_to_account(
            source_ptr,
            source_size,
            target_ptr,
            target_size,
            amount_ptr,
            amount_size,
        )
    };
    TransferredTo::result_from(return_code)
}

/// Transfers `amount` of motes from `source` purse to `target` purse.
pub fn transfer_from_purse_to_purse(
    source: PurseId,
    target: PurseId,
    amount: U512,
) -> Result<(), Error> {
    let (source_ptr, source_size, _bytes) = to_ptr(&source);
    let (target_ptr, target_size, _bytes) = to_ptr(&target);
    let (amount_ptr, amount_size, _bytes) = to_ptr(&amount);
    let result = unsafe {
        ext_ffi::transfer_from_purse_to_purse(
            source_ptr,
            source_size,
            target_ptr,
            target_size,
            amount_ptr,
            amount_size,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(Error::Transfer)
    }
}
