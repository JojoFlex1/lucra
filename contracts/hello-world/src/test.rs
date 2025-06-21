#[cfg(test)]
mod test {
    use super::*;
    use soroban_sdk::{testutils::{Address as _, AuthorizedFunction, AuthorizedInvocation}, Env, Address};

    #[test]
    fn test_initialize() {
        let env = Env::default();
        env.mock_all_auths();
        
        let contract_id = env.register_contract(None, DustAggregator);
        let client = DustAggregatorClient::new(&env, &contract_id);
        
        let admin = Address::generate(&env);
        
        client.initialize(&admin, &100); // 1% fee
        
        assert_eq!(client.get_admin(), admin);
        assert_eq!(client.get_fee_rate(), 100);
        assert!(!client.is_paused());
    }

    #[test]
    fn test_deposit_and_withdraw() {
        let env = Env::default();
        env.mock_all_auths();
        
        let contract_id = env.register_contract(None, DustAggregator);
        let client = DustAggregatorClient::new(&env, &contract_id);
        
        let admin = Address::generate(&env);
        let user = Address::generate(&env);
        let token = Address::generate(&env);
        
        client.initialize(&admin, &0);
        
        // Mock token contract would be needed for full test
        // This is a simplified test structure
        
        // Test would verify deposit and withdrawal functionality
        // In practice, you'd need to deploy a test token contract
    }

    #[test]
    fn test_pause_functionality() {
        let env = Env::default();
        env.mock_all_auths();
        
        let contract_id = env.register_contract(None, DustAggregator);
        let client = DustAggregatorClient::new(&env, &contract_id);
        
        let admin = Address::generate(&env);
        
        client.initialize(&admin, &0);
        assert!(!client.is_paused());
        
        client.set_paused(&true);
        assert!(client.is_paused());
        
        client.set_paused(&false);
        assert!(!client.is_paused());
    }
}
