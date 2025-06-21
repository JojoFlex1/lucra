#![no_std]
use soroban_sdk::{
    contract, contractimpl, contracttype, Address, Env, Vec, Symbol, 
    token, log, contracterror
};

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum DustError {
    ArrayLengthMismatch = 1,
    InsufficientBalance = 2,
    InvalidAmount = 3,
    TransferFailed = 4,
    UnauthorizedAccess = 5,
    ContractPaused = 6,
    NotInitialized = 7,
    AlreadyInitialized = 8,
    InvalidSlippage = 9,
    ZeroAddress = 10,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DustEvent {
    Deposit,
    BatchDeposit,
    Swap,
    Withdraw,
    BatchWithdraw,
    AdminChanged,
    ContractPaused,
    ContractUnpaused,
    EmergencyWithdraw,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UserBalance {
    pub token: Address,
    pub balance: i128,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SwapParams {
    pub tokens_in: Vec<Address>,
    pub amounts_in: Vec<i128>,
    pub token_out: Address,
    pub min_amount_out: i128,
    pub slippage_tolerance: u32, // basis points (100 = 1%)
}

#[contract]
pub struct DustAggregator;

const ADMIN_KEY: &str = "admin";
const INITIALIZED_KEY: &str = "initialized";
const PAUSED_KEY: &str = "paused";
const FEE_RATE_KEY: &str = "fee_rate";
const USER_TOKENS_KEY: &str = "user_tokens";

#[contractimpl]
impl DustAggregator {
    /// Initialize the contract with admin address and fee rate
    pub fn initialize(env: Env, admin: Address, fee_rate: u32) -> Result<(), DustError> {
        // Ensure contract is only initialized once
        if env.storage().instance().has(&Symbol::new(&env, INITIALIZED_KEY)) {
            return Err(DustError::AlreadyInitialized);
        }
        
        // Validate inputs
        if admin == env.current_contract_address() {
            return Err(DustError::ZeroAddress);
        }
        
        if fee_rate > 10000 { // Max 100% fee
            return Err(DustError::InvalidAmount);
        }
        
        admin.require_auth();
        
        env.storage().instance().set(&Symbol::new(&env, ADMIN_KEY), &admin);
        env.storage().instance().set(&Symbol::new(&env, INITIALIZED_KEY), &true);
        env.storage().instance().set(&Symbol::new(&env, FEE_RATE_KEY), &fee_rate);
        env.storage().instance().set(&Symbol::new(&env, PAUSED_KEY), &false);
        
        log!(&env, "Contract initialized with admin: {:?}", admin);
        Ok(())
    }

    /// Deposit a single token
    pub fn deposit(env: Env, from: Address, token: Address, amount: i128) -> Result<(), DustError> {
        Self::require_not_paused(&env)?;
        Self::require_initialized(&env)?;
        
        from.require_auth();
        
        if amount <= 0 {
            return Err(DustError::InvalidAmount);
        }

        let key = (from.clone(), token.clone());
        let token_client = token::Client::new(&env, &token);
        
        // Check user's balance before transfer
        let user_balance = token_client.balance(&from);
        if user_balance < amount {
            return Err(DustError::InsufficientBalance);
        }

        // Perform transfer
        token_client.transfer(&from, &env.current_contract_address(), &amount);
        
        // Update internal balance
        let current_balance = Self::get_internal(&env, &from, &token);
        let new_balance = current_balance.checked_add(amount)
            .ok_or(DustError::InvalidAmount)?;
        
        env.storage().persistent().set(&key, &new_balance);
        
        // Track user's tokens
        Self::add_user_token(&env, &from, &token);
        
        // Emit event
        env.events().publish((DustEvent::Deposit, from.clone()), (token, amount));
        
        log!(&env, "Deposited {} tokens for user", amount);
        Ok(())
    }

    /// Deposit multiple tokens in one transaction
    pub fn deposit_batch(env: Env, from: Address, tokens: Vec<Address>, amounts: Vec<i128>) -> Result<(), DustError> {
        Self::require_not_paused(&env)?;
        Self::require_initialized(&env)?;
        
        from.require_auth();
        
        // Validate input arrays
        if tokens.len() != amounts.len() || tokens.len() == 0 {
            return Err(DustError::ArrayLengthMismatch);
        }

        // Validate all amounts first
        for i in 0..amounts.len() {
            let amount = amounts.get_unchecked(i);
            if amount <= 0 {
                return Err(DustError::InvalidAmount);
            }
        }

        // Process deposits
        for i in 0..tokens.len() {
            let token = tokens.get_unchecked(i);
            let amount = amounts.get_unchecked(i);
            
            let key = (from.clone(), token.clone());
            let token_client = token::Client::new(&env, &token);
            
            // Check balance and transfer
            let user_balance = token_client.balance(&from);
            if user_balance < amount {
                return Err(DustError::InsufficientBalance);
            }
            
            token_client.transfer(&from, &env.current_contract_address(), &amount);
            
            // Update balance
            let current_balance = Self::get_internal(&env, &from, &token);
            let new_balance = current_balance.checked_add(amount)
                .ok_or(DustError::InvalidAmount)?;
            
            env.storage().persistent().set(&key, &new_balance);
            
            // Track user's tokens
            Self::add_user_token(&env, &from, &token);
        }
        
        // Emit batch event
        env.events().publish((DustEvent::BatchDeposit, from.clone()), tokens.len());
        
        log!(&env, "Batch deposited {} tokens for user", tokens.len());
        Ok(())
    }

    /// Swap tokens with constant exchange rates for simplicity
    pub fn swap(env: Env, from: Address, swap_params: SwapParams) -> Result<(), DustError> {
        Self::require_not_paused(&env)?;
        Self::require_initialized(&env)?;
        
        from.require_auth();
        
        if swap_params.tokens_in.len() != swap_params.amounts_in.len() || swap_params.tokens_in.len() == 0 {
            return Err(DustError::ArrayLengthMismatch);
        }

        if swap_params.slippage_tolerance > 10000 {
            return Err(DustError::InvalidSlippage);
        }

        let mut total_value: i128 = 0;
        
        // Process input tokens
        for i in 0..swap_params.tokens_in.len() {
            let token = swap_params.tokens_in.get_unchecked(i);
            let amount = swap_params.amounts_in.get_unchecked(i);
            
            if amount <= 0 {
                return Err(DustError::InvalidAmount);
            }
            
            let key = (from.clone(), token.clone());
            let current_balance = Self::get_internal(&env, &from, &token);
            
            if current_balance < amount {
                return Err(DustError::InsufficientBalance);
            }
            
            // Deduct from balance
            let new_balance = current_balance - amount;
            env.storage().persistent().set(&key, &new_balance);
            
            // Get exchange rate and calculate value
            let exchange_rate = Self::get_exchange_rate(&env, &token, &swap_params.token_out);
            total_value += (amount * exchange_rate) / 10000; // 4 decimal precision
        }
        
        // Apply fee
        let fee_rate: u32 = env.storage().instance()
            .get(&Symbol::new(&env, FEE_RATE_KEY))
            .unwrap_or(0);
        
        let fee_amount = (total_value * fee_rate as i128) / 10000;
        let final_amount = total_value - fee_amount;
        
        // Check minimum output requirement with slippage
        if final_amount < swap_params.min_amount_out {
            return Err(DustError::InsufficientBalance);
        }
        
        // Add to output token balance
        let out_key = (from.clone(), swap_params.token_out.clone());
        let current_out_balance = Self::get_internal(&env, &from, &swap_params.token_out);
        let new_out_balance = current_out_balance.checked_add(final_amount)
            .ok_or(DustError::InvalidAmount)?;
        
        env.storage().persistent().set(&out_key, &new_out_balance);
        
        // Track user's new token
        Self::add_user_token(&env, &from, &swap_params.token_out);
        
        // Emit swap event
        env.events().publish((DustEvent::Swap, from.clone()), (swap_params.token_out, final_amount));
        
        log!(&env, "Swapped tokens for user, output: {}, fee: {}", final_amount, fee_amount);
        Ok(())
    }

    /// Withdraw a single token
    pub fn withdraw(env: Env, to: Address, token: Address, amount: i128) -> Result<(), DustError> {
        Self::require_not_paused(&env)?;
        Self::require_initialized(&env)?;
        
        to.require_auth();
        
        if amount <= 0 {
            return Err(DustError::InvalidAmount);
        }
        
        let key = (to.clone(), token.clone());
        let current_balance = Self::get_internal(&env, &to, &token);
        
        if current_balance < amount {
            return Err(DustError::InsufficientBalance);
        }
        
        // Update balance
        let new_balance = current_balance - amount;
        env.storage().persistent().set(&key, &new_balance);
        
        // Transfer tokens back to user
        let token_client = token::Client::new(&env, &token);
        token_client.transfer(&env.current_contract_address(), &to, &amount);
        
        // Emit event
        env.events().publish((DustEvent::Withdraw, to.clone()), (token, amount));
        
        log!(&env, "Withdrawn {} tokens for user", amount);
        Ok(())
    }

    /// Withdraw multiple tokens at once
    pub fn withdraw_batch(env: Env, to: Address, tokens: Vec<Address>, amounts: Vec<i128>) -> Result<(), DustError> {
        Self::require_not_paused(&env)?;
        Self::require_initialized(&env)?;
        
        to.require_auth();
        
        if tokens.len() != amounts.len() || tokens.len() == 0 {
            return Err(DustError::ArrayLengthMismatch);
        }

        for i in 0..tokens.len() {
            let token = tokens.get_unchecked(i);
            let amount = amounts.get_unchecked(i);
            
            if amount <= 0 {
                return Err(DustError::InvalidAmount);
            }
            
            let key = (to.clone(), token.clone());
            let current_balance = Self::get_internal(&env, &to, &token);
            
            if current_balance < amount {
                return Err(DustError::InsufficientBalance);
            }
            
            // Update balance
            let new_balance = current_balance - amount;
            env.storage().persistent().set(&key, &new_balance);
            
            // Transfer tokens back to user
            let token_client = token::Client::new(&env, &token);
            token_client.transfer(&env.current_contract_address(), &to, &amount);
        }
        
        env.events().publish((DustEvent::BatchWithdraw, to.clone()), tokens.len());
        log!(&env, "Batch withdrawn {} tokens for user", tokens.len());
        Ok(())
    }

    /// Emergency withdraw all tokens for a user (admin function)
    pub fn emergency_withdraw(env: Env, user: Address) -> Result<(), DustError> {
        Self::require_initialized(&env)?;
        
        let admin: Address = env.storage().instance()
            .get(&Symbol::new(&env, ADMIN_KEY))
            .ok_or(DustError::UnauthorizedAccess)?;
        
        admin.require_auth();
        
        let user_tokens = Self::get_user_tokens(&env, &user);
        
        for token in user_tokens.iter() {
            let balance = Self::get_internal(&env, &user, &token);
            
            if balance > 0 {
                let key = (user.clone(), token.clone());
                env.storage().persistent().set(&key, &0i128);
                
                let token_client = token::Client::new(&env, &token);
                token_client.transfer(&env.current_contract_address(), &user, &balance);
            }
        }
        
        env.events().publish((DustEvent::EmergencyWithdraw, user.clone()), user_tokens.len());
        log!(&env, "Emergency withdraw completed for user");
        Ok(())
    }

    /// Get balance for a specific token
    pub fn get_balance(env: Env, user: Address, token: Address) -> i128 {
        Self::get_internal(&env, &user, &token)
    }

    /// Get all token balances for a user
    pub fn get_all_balances(env: Env, user: Address) -> Vec<UserBalance> {
        let user_tokens = Self::get_user_tokens(&env, &user);
        let mut balances = Vec::new(&env);
        
        for token in user_tokens.iter() {
            let balance = Self::get_internal(&env, &user, &token);
            if balance > 0 {
                balances.push_back(UserBalance {
                    token: token.clone(),
                    balance,
                });
            }
        }
        
        balances
    }

    /// Get portfolio value with constant exchange rates
    pub fn get_portfolio_value_usd(env: Env, user: Address) -> i128 {
        let user_tokens = Self::get_user_tokens(&env, &user);
        let mut total_value: i128 = 0;
        
        for token in user_tokens.iter() {
            let balance = Self::get_internal(&env, &user, &token);
            if balance > 0 {
                // Use a constant USD value for simplicity (you can adjust these rates)
                let usd_rate = Self::get_usd_rate(&env, &token);
                let token_value = (balance * usd_rate) / 10000; // 4 decimal precision
                total_value += token_value;
            }
        }
        
        total_value
    }

    /// Admin function to set pause state
    pub fn set_paused(env: Env, paused: bool) -> Result<(), DustError> {
        Self::require_initialized(&env)?;
        
        let admin: Address = env.storage().instance()
            .get(&Symbol::new(&env, ADMIN_KEY))
            .ok_or(DustError::UnauthorizedAccess)?;
        
        admin.require_auth();
        env.storage().instance().set(&Symbol::new(&env, PAUSED_KEY), &paused);
        
        let event = if paused { DustEvent::ContractPaused } else { DustEvent::ContractUnpaused };
        env.events().publish((event, admin), paused);
        
        log!(&env, "Contract pause state changed to: {}", paused);
        Ok(())
    }

    /// Check if contract is paused
    pub fn is_paused(env: Env) -> bool {
        env.storage().instance()
            .get(&Symbol::new(&env, PAUSED_KEY))
            .unwrap_or(false)
    }

    /// Admin function to change admin
    pub fn change_admin(env: Env, new_admin: Address) -> Result<(), DustError> {
        Self::require_initialized(&env)?;
        
        let current_admin: Address = env.storage().instance()
            .get(&Symbol::new(&env, ADMIN_KEY))
            .ok_or(DustError::UnauthorizedAccess)?;
        
        current_admin.require_auth();
        new_admin.require_auth();
        
        env.storage().instance().set(&Symbol::new(&env, ADMIN_KEY), &new_admin);
        
        env.events().publish((DustEvent::AdminChanged, new_admin.clone()), current_admin);
        log!(&env, "Admin changed to: {:?}", new_admin);
        Ok(())
    }

    /// Admin function to set fee rate
    pub fn set_fee_rate(env: Env, fee_rate: u32) -> Result<(), DustError> {
        Self::require_initialized(&env)?;
        
        let admin: Address = env.storage().instance()
            .get(&Symbol::new(&env, ADMIN_KEY))
            .ok_or(DustError::UnauthorizedAccess)?;
        
        admin.require_auth();
        
        if fee_rate > 10000 {
            return Err(DustError::InvalidAmount);
        }
        
        env.storage().instance().set(&Symbol::new(&env, FEE_RATE_KEY), &fee_rate);
        log!(&env, "Fee rate changed to: {}", fee_rate);
        Ok(())
    }

    /// Get current fee rate
    pub fn get_fee_rate(env: Env) -> u32 {
        env.storage().instance()
            .get(&Symbol::new(&env, FEE_RATE_KEY))
            .unwrap_or(0)
    }

    /// Get contract admin
    pub fn get_admin(env: Env) -> Result<Address, DustError> {
        env.storage().instance()
            .get(&Symbol::new(&env, ADMIN_KEY))
            .ok_or(DustError::NotInitialized)
    }

    // Internal helper functions
    
    /// Get internal balance
    fn get_internal(env: &Env, user: &Address, token: &Address) -> i128 {
        let key = (user.clone(), token.clone());
        env.storage().persistent().get(&key).unwrap_or(0)
    }

    /// Check if contract is not paused
    fn require_not_paused(env: &Env) -> Result<(), DustError> {
        if env.storage().instance().get(&Symbol::new(env, PAUSED_KEY)).unwrap_or(false) {
            Err(DustError::ContractPaused)
        } else {
            Ok(())
        }
    }

    /// Check if contract is initialized
    fn require_initialized(env: &Env) -> Result<(), DustError> {
        if !env.storage().instance().get(&Symbol::new(env, INITIALIZED_KEY)).unwrap_or(false) {
            Err(DustError::NotInitialized)
        } else {
            Ok(())
        }
    }

    /// Add token to user's token list
    fn add_user_token(env: &Env, user: &Address, token: &Address) {
        let key = (Symbol::new(env, USER_TOKENS_KEY), user.clone());
        let mut user_tokens: Vec<Address> = env.storage().persistent().get(&key).unwrap_or(Vec::new(env));
        
        // Check if token already exists
        for existing_token in user_tokens.iter() {
            if existing_token == *token {
                return; // Token already tracked
            }
        }
        
        user_tokens.push_back(token.clone());
        env.storage().persistent().set(&key, &user_tokens);
    }

    /// Get user's token list
    fn get_user_tokens(env: &Env, user: &Address) -> Vec<Address> {
        let key = (Symbol::new(env, USER_TOKENS_KEY), user.clone());
        env.storage().persistent().get(&key).unwrap_or(Vec::new(env))
    }

    /// Get exchange rate between two tokens using constant rates
    fn get_exchange_rate(env: &Env, token_in: &Address, token_out: &Address) -> i128 {
        if token_in == token_out {
            return 10000; // 1:1 ratio
        }
        
        // Simple token identification without format! macro
        let from_symbol = Self::identify_token_symbol(env, token_in);
        let to_symbol = Self::identify_token_symbol(env, token_out);
        
        // Define exchange rates (you can modify these as needed)
        // Format: rate * 10000 (so 1.5 = 15000)
        
        match (from_symbol, to_symbol) {
            // BTC rates
            ("BTC", "USD") => 45000_0000i128,  // 1 BTC = $45,000
            ("BTC", "ETH") => 15_0000i128,     // 1 BTC = 15 ETH
            ("BTC", "XLM") => 300000_0000i128, // 1 BTC = 300,000 XLM
            ("BTC", "USDC") => 45000_0000i128, // 1 BTC = $45,000 USDC
            
            // ETH rates
            ("ETH", "USD") => 3000_0000i128,   // 1 ETH = $3,000
            ("ETH", "BTC") => 667i128,         // 1 ETH = 0.0667 BTC
            ("ETH", "XLM") => 20000_0000i128,  // 1 ETH = 20,000 XLM
            ("ETH", "USDC") => 3000_0000i128,  // 1 ETH = $3,000 USDC
            
            // XLM rates
            ("XLM", "USD") => 15i128,          // 1 XLM = $0.15
            ("XLM", "BTC") => 3i128,           // 1 XLM = 0.0003 BTC
            ("XLM", "ETH") => 5i128,           // 1 XLM = 0.005 ETH
            ("XLM", "USDC") => 15i128,         // 1 XLM = $0.15 USDC
            
            // USDC rates
            ("USDC", "USD") => 10000i128,      // 1 USDC = $1.00
            ("USDC", "BTC") => 2i128,          // 1 USDC = 0.002 BTC
            ("USDC", "ETH") => 3i128,          // 1 USDC = 0.003 ETH
            ("USDC", "XLM") => 667i128,        // 1 USDC = 6.67 XLM
            
            // Default rate if not found (treat as 1:1)
            _ => 10000,
        }
    }
    
    /// Get USD rate for a token
    fn get_usd_rate(env: &Env, token: &Address) -> i128 {
        let symbol = Self::identify_token_symbol(env, token);
        
        match symbol {
            "BTC" => 45000_0000i128,  // $45,000
            "ETH" => 3000_0000i128,   // $3,000
            "XLM" => 15i128,          // $0.15
            "USDC" => 10000i128,      // $1.00
            _ => 10000i128,           // Default to $1.00
        }
    }
/// Simple token symbol identification from address
/// Simple token symbol identification from address
fn identify_token_symbol(_env: &Env, _token: &Address) -> &'static str {
    "XLM"
}
}
