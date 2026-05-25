//! Account models used by the order system.
//!
//! The first supported account model is a plain cash account. It tracks cash
//! only and intentionally leaves position valuation to the caller, because
//! marked-to-market exposure depends on external prices and the current
//! holdings.

use rust_decimal::Decimal;
use thiserror::Error;

use crate::order_sys::AccountId;

/// High-level account variants supported by the order system.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Account {
    /// Plain cash account with no margin or leverage semantics.
    Cash(CashAccount),
}

impl Account {
    /// Creates a cash-only account.
    pub fn new_cash(
        id: AccountId,
        base_currency: impl Into<String>,
        cash_balance: Decimal,
    ) -> Result<Self, AccountError> {
        Ok(Self::Cash(CashAccount::new(
            id,
            base_currency,
            cash_balance,
        )?))
    }

    /// Returns the account identifier.
    pub fn id(&self) -> AccountId {
        match self {
            Self::Cash(account) => account.id(),
        }
    }

    /// Returns the account model.
    pub fn kind(&self) -> AccountKind {
        match self {
            Self::Cash(_) => AccountKind::Cash,
        }
    }

    /// Returns the account base currency.
    pub fn base_currency(&self) -> &str {
        match self {
            Self::Cash(account) => account.base_currency(),
        }
    }

    /// Returns the current cash balance.
    pub fn cash_balance(&self) -> Decimal {
        match self {
            Self::Cash(account) => account.cash_balance(),
        }
    }

    /// Returns total equity using externally supplied marked position value.
    pub fn equity(&self, marked_positions_value: Decimal) -> Decimal {
        match self {
            Self::Cash(account) => account.equity(marked_positions_value),
        }
    }

    /// Returns a shared reference to the inner cash account if this is one.
    pub fn as_cash(&self) -> Option<&CashAccount> {
        match self {
            Self::Cash(account) => Some(account),
        }
    }

    /// Returns a mutable reference to the inner cash account if this is one.
    pub fn as_cash_mut(&mut self) -> Option<&mut CashAccount> {
        match self {
            Self::Cash(account) => Some(account),
        }
    }
}

/// Account model families supported by the system.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccountKind {
    /// Cash-only account with no leverage or margin semantics.
    Cash,
}

/// Errors raised while creating or mutating accounts.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum AccountError {
    /// Base currency code must not be blank.
    #[error("account base currency cannot be empty")]
    EmptyBaseCurrency,
    /// Cash balance must not be negative when constructing an account.
    #[error("cash balance cannot be negative, got {cash_balance}")]
    NegativeCashBalance {
        /// Invalid initial cash balance.
        cash_balance: Decimal,
    },
    /// The requested mutation would push the cash account below zero.
    #[error(
        "cash balance would become negative: current {current_balance}, delta {delta}, attempted {attempted_balance}"
    )]
    CashBalanceWouldBecomeNegative {
        /// Current cash balance before applying the delta.
        current_balance: Decimal,
        /// Requested cash delta.
        delta: Decimal,
        /// Resulting negative balance that was rejected.
        attempted_balance: Decimal,
    },
}

/// Plain cash account with one cash balance and one base currency.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CashAccount {
    /// Stable account identifier.
    id: AccountId,
    /// Account reporting currency, for example `USD`.
    base_currency: String,
    /// Current cash balance.
    cash_balance: Decimal,
}

impl CashAccount {
    /// Creates one cash account.
    pub fn new(
        id: AccountId,
        base_currency: impl Into<String>,
        cash_balance: Decimal,
    ) -> Result<Self, AccountError> {
        let base_currency = base_currency.into();
        if base_currency.trim().is_empty() {
            return Err(AccountError::EmptyBaseCurrency);
        }
        if cash_balance < Decimal::ZERO {
            return Err(AccountError::NegativeCashBalance { cash_balance });
        }

        Ok(Self {
            id,
            base_currency,
            cash_balance,
        })
    }

    /// Returns the account identifier.
    pub fn id(&self) -> AccountId {
        self.id
    }

    /// Returns the account base currency.
    pub fn base_currency(&self) -> &str {
        &self.base_currency
    }

    /// Returns the current cash balance.
    pub fn cash_balance(&self) -> Decimal {
        self.cash_balance
    }

    /// Returns total equity using externally supplied marked position value.
    ///
    /// For a cash account, equity is the sum of available cash and the current
    /// marked value of any open positions tracked elsewhere.
    pub fn equity(&self, marked_positions_value: Decimal) -> Decimal {
        self.cash_balance + marked_positions_value
    }

    /// Applies a signed cash movement while preserving the non-negative balance invariant.
    pub fn apply_cash_delta(&mut self, delta: Decimal) -> Result<(), AccountError> {
        let attempted_balance = self.cash_balance + delta;
        if attempted_balance < Decimal::ZERO {
            return Err(AccountError::CashBalanceWouldBecomeNegative {
                current_balance: self.cash_balance,
                delta,
                attempted_balance,
            });
        }

        self.cash_balance = attempted_balance;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use rust_decimal::Decimal;

    use super::{Account, AccountError, AccountKind, CashAccount};
    use crate::order_sys::AccountId;

    fn dec(value: i64, scale: u32) -> Decimal {
        Decimal::new(value, scale)
    }

    #[test]
    fn cash_account_rejects_blank_currency_and_negative_balance() {
        assert_eq!(
            CashAccount::new(AccountId::new(1), "   ", Decimal::ZERO).unwrap_err(),
            AccountError::EmptyBaseCurrency
        );
        assert_eq!(
            CashAccount::new(AccountId::new(1), "USD", dec(-1, 0)).unwrap_err(),
            AccountError::NegativeCashBalance {
                cash_balance: dec(-1, 0),
            }
        );
    }

    #[test]
    fn cash_account_applies_cash_deltas_without_going_negative() {
        let mut account = CashAccount::new(AccountId::new(1), "USD", dec(1000, 0)).unwrap();

        account.apply_cash_delta(dec(-250, 0)).unwrap();
        account.apply_cash_delta(dec(125, 0)).unwrap();

        assert_eq!(account.cash_balance(), dec(875, 0));
        assert_eq!(account.equity(dec(325, 0)), dec(1200, 0));

        let err = account.apply_cash_delta(dec(-900, 0)).unwrap_err();
        assert_eq!(
            err,
            AccountError::CashBalanceWouldBecomeNegative {
                current_balance: dec(875, 0),
                delta: dec(-900, 0),
                attempted_balance: dec(-25, 0),
            }
        );
    }

    #[test]
    fn account_enum_delegates_cash_behavior() {
        let mut account = Account::new_cash(AccountId::new(7), "USD", dec(500, 0)).unwrap();

        assert_eq!(account.id(), AccountId::new(7));
        assert_eq!(account.kind(), AccountKind::Cash);
        assert_eq!(account.base_currency(), "USD");
        assert_eq!(account.cash_balance(), dec(500, 0));
        assert_eq!(account.equity(dec(40, 0)), dec(540, 0));

        account
            .as_cash_mut()
            .expect("cash account variant must expose mutable access")
            .apply_cash_delta(dec(-100, 0))
            .unwrap();

        assert_eq!(account.cash_balance(), dec(400, 0));
    }
}
