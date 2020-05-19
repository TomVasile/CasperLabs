use num_traits::identities::Zero;

use engine_core::engine_state::{
    genesis::{GenesisAccount, GenesisConfig},
    run_genesis_request::RunGenesisRequest,
    CONV_RATE,
};

use engine_shared::motes::Motes;
use types::{AccessRights, Key, URef, U512};

use crate::{
    internal::{InMemoryWasmTestBuilder, DEFAULT_GENESIS_CONFIG, DEFAULT_GENESIS_CONFIG_HASH},
    Error, PublicKey, Result, Session, URefAddr, Value,
};

/// Context in which to run a test of a Wasm smart contract.
pub struct TestContext {
    inner: InMemoryWasmTestBuilder,
}

impl TestContext {
    fn maybe_get_balance_for_purse(&self, purse_address: Option<URef>) -> Option<Motes> {
        match purse_address {
            None => None,
            Some(purse_address) => {
                let purse_balance = self.get_balance(purse_address.addr());
                Some(Motes::new(purse_balance))
            }
        }
    }
    /// Runs the supplied [`Session`], asserting successful execution of the contained deploy
    ///
    /// if Session.expect_success (default) will panic if failure.  (Allows cases where failure is
    /// expected) if Session.check_transfer_success is given, will verify transfer balances
    /// including gas used. if Session.commit (default) will commit resulting transforms.
    pub fn run(&mut self, session: Session) -> &mut Self {
        match session.check_transfer_success {
            Some(session_transfer_info) => {
                let source_initial_balance = self
                    .maybe_get_balance_for_purse(Some(session_transfer_info.source_purse))
                    .expect("source purse balance");
                let maybe_target_initial_balance =
                    self.maybe_get_balance_for_purse(session_transfer_info.maybe_target_purse);

                let builder = self.inner.exec(session.inner);
                if session.expect_success {
                    builder.expect_success();
                }
                if session.commit {
                    builder.commit();
                }

                let gas_cost = builder.last_exec_gas_cost();
                match maybe_target_initial_balance {
                    None => (),
                    Some(target_initial_balance) => {
                        let target_ending_balance = self
                            .maybe_get_balance_for_purse(session_transfer_info.maybe_target_purse)
                            .expect("target ending balance");

                        assert_eq!(
                            target_initial_balance + session_transfer_info.transfer_amount,
                            target_ending_balance,
                            "incorrect target balance"
                        )
                    }
                }

                let source_ending_balance = self
                    .maybe_get_balance_for_purse(Some(session_transfer_info.source_purse))
                    .expect("source ending balance");
                assert_eq!(
                    source_initial_balance
                        - session_transfer_info.transfer_amount
                        - Motes::from_gas(gas_cost, CONV_RATE).expect("motes from gas"),
                    source_ending_balance,
                    "incorrect source balance"
                );
            }
            None => {
                let builder = self.inner.exec(session.inner);
                if session.expect_success {
                    builder.expect_success();
                }
                if session.commit {
                    builder.commit();
                }
            }
        }

        self
    }

    /// Queries for a [`Value`] stored under the given `key` and `path`.
    ///
    /// Returns an [`Error`] if not found.
    pub fn query<T: AsRef<str>>(&self, key: PublicKey, path: &[T]) -> Result<Value> {
        let path = path.iter().map(AsRef::as_ref).collect::<Vec<_>>();
        self.inner
            .query(None, Key::Account(key), &path)
            .map(Value::new)
            .map_err(Error::from)
    }

    /// Gets the balance of the purse under the given [`URefAddr`].
    ///
    /// Note that this requires performing an earlier query to retrieve `purse_addr`.
    pub fn get_balance(&self, purse_addr: URefAddr) -> U512 {
        let purse = URef::new(purse_addr, AccessRights::READ);
        self.inner.get_purse_balance(purse)
    }

    /// Gets the main purse Uref from an account
    pub fn get_main_purse_address(&self, account_key: PublicKey) -> Option<URef> {
        match self.inner.get_account(account_key) {
            None => None,
            Some(account) => Some(account.main_purse()),
        }
    }
}

/// Builder for a [`TestContext`].
pub struct TestContextBuilder {
    genesis_config: GenesisConfig,
}

impl TestContextBuilder {
    /// Constructs a new `TestContextBuilder` initialised with default values for an account, i.e.
    /// an account at [`DEFAULT_ACCOUNT_ADDR`](crate::DEFAULT_ACCOUNT_ADDR) with an initial balance
    /// of [`DEFAULT_ACCOUNT_INITIAL_BALANCE`](crate::DEFAULT_ACCOUNT_INITIAL_BALANCE) which will be
    /// added to the Genesis block.
    pub fn new() -> Self {
        TestContextBuilder {
            genesis_config: DEFAULT_GENESIS_CONFIG.clone(),
        }
    }

    /// Returns `self` with the provided account's details added to existing ones, for inclusion in
    /// the Genesis block.
    ///
    /// Note: `initial_balance` represents the number of motes.
    pub fn with_account(mut self, address: PublicKey, initial_balance: U512) -> Self {
        let new_account = GenesisAccount::new(address, Motes::new(initial_balance), Motes::zero());
        self.genesis_config
            .ee_config_mut()
            .push_account(new_account);
        self
    }

    /// Builds the [`TestContext`].
    pub fn build(self) -> TestContext {
        let mut inner = InMemoryWasmTestBuilder::default();
        let run_genesis_request = RunGenesisRequest::new(
            *DEFAULT_GENESIS_CONFIG_HASH,
            self.genesis_config.protocol_version(),
            self.genesis_config.take_ee_config(),
        );
        inner.run_genesis(&run_genesis_request);
        TestContext { inner }
    }
}

impl Default for TestContextBuilder {
    fn default() -> Self {
        TestContextBuilder::new()
    }
}
