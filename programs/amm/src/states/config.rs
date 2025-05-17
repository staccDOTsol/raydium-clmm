use crate::error::ErrorCode;
use anchor_lang::prelude::*;

pub const AMM_CONFIG_SEED: &str = "amm_config";

pub const FEE_RATE_DENOMINATOR_VALUE: u32 = 1_000_000;
/// Default flat trade fee charged on each swap
pub const TRADE_FLAT_FEE_DEFAULT: u64 = 100_000;

/// Holds the current owner of the factory
#[account]
#[derive(Debug)]
pub struct AmmConfig {
    /// Bump to identify PDA
    pub bump: u8,
    pub index: u16,
    /// Address of the protocol owner
    pub owner: Pubkey,
    /// The protocol fee
    pub protocol_fee_rate: u32,
    /// Flat trade fee charged on each swap
    pub trade_fee_flat: u64,
    /// The tick spacing
    pub tick_spacing: u16,
    /// The fund fee, denominated in hundredths of a bip (10^-6)
    pub fund_fee_rate: u32,
    // padding space for upgrade
    pub padding_u32: u32,
    pub fund_owner: Pubkey,
    pub padding: [u64; 3],
}

impl Default for AmmConfig {
    fn default() -> Self {
        Self {
            bump: 0,
            index: 0,
            owner: Pubkey::default(),
            protocol_fee_rate: 0,
            trade_fee_flat: TRADE_FLAT_FEE_DEFAULT,
            tick_spacing: 0,
            fund_fee_rate: 0,
            padding_u32: 0,
            fund_owner: Pubkey::default(),
            padding: [0u64; 3],
        }
    }
}

impl AmmConfig {
    pub const LEN: usize = 8 + 1 + 2 + 32 + 4 + 8 + 2 + 64;

    pub fn is_authorized<'info>(
        &self,
        signer: &Signer<'info>,
        expect_pubkey: Pubkey,
    ) -> Result<()> {
        require!(
            signer.key() == self.owner || expect_pubkey == signer.key(),
            ErrorCode::NotApproved
        );
        Ok(())
    }
}

/// Emitted when create or update a config
#[event]
#[cfg_attr(feature = "client", derive(Debug))]
pub struct ConfigChangeEvent {
    pub index: u16,
    pub owner: Pubkey,
    pub protocol_fee_rate: u32,
    pub trade_fee_flat: u64,
    pub tick_spacing: u16,
    pub fund_fee_rate: u32,
    pub fund_owner: Pubkey,
}
