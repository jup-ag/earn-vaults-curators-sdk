use offerbook_client::prelude::*;

#[test]
fn yield_builders_and_events_remain_accessible() {
    assert_ne!(
        instructions::EscrowYieldDeposit::DISCRIMINATOR,
        state::EscrowYieldDeposit::DISCRIMINATOR
    );
    assert_ne!(
        instructions::EscrowYieldWithdraw::DISCRIMINATOR,
        state::EscrowYieldWithdraw::DISCRIMINATOR
    );
}
