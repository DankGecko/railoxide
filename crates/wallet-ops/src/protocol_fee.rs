use alloy::primitives::{U256, uint};

pub const RAILGUN_PROTOCOL_FEE_BPS: U256 = uint!(25_U256);
pub(crate) const FEE_BASIS_POINTS_DENOMINATOR: U256 = uint!(10_000_U256);

#[must_use]
pub(crate) fn railgun_protocol_fee_amount(amount: U256, fee_bps: U256) -> U256 {
    amount * fee_bps / FEE_BASIS_POINTS_DENOMINATOR
}

#[must_use]
pub fn format_protocol_fee_percentage(fee_bps: U256) -> String {
    let whole = fee_bps / U256::from(100);
    let fractional = fee_bps % U256::from(100);
    if fractional.is_zero() {
        return format!("{whole}%");
    }

    let mut fractional = format!("{fractional:0>2}");
    while fractional.ends_with('0') {
        fractional.pop();
    }
    format!("{whole}.{fractional}%")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_fee_percentage_formats_basis_points() {
        assert_eq!(format_protocol_fee_percentage(uint!(25_U256)), "0.25%");
        assert_eq!(format_protocol_fee_percentage(uint!(250_U256)), "2.5%");
        assert_eq!(format_protocol_fee_percentage(uint!(100_U256)), "1%");
    }
}
