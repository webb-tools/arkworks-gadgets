use super::{sbox::constraints::SboxConstraints, PoseidonParameters, Rounds, CRH};
use crate::utils::to_field_var_elements;
use ark_crypto_primitives::crh::constraints::{CRHGadget as CRHGadgetTrait, TwoToOneCRHGadget};
use ark_ff::PrimeField;
use ark_r1cs_std::{
	alloc::AllocVar,
	fields::{fp::FpVar, FieldVar},
	prelude::*,
	uint8::UInt8,
};
use ark_relations::r1cs::{Namespace, SynthesisError};
use ark_std::{marker::PhantomData, vec::Vec};
use core::borrow::Borrow;

#[derive(Default, Clone)]
pub struct PoseidonParametersVar<F: PrimeField> {
	/// The round key constants
	pub round_keys: Vec<FpVar<F>>,
	/// The MDS matrix to apply in the mix layer.
	pub mds_matrix: Vec<Vec<FpVar<F>>>,
}

pub struct CRHGadget<F: PrimeField, P: Rounds> {
	field: PhantomData<F>,
	params: PhantomData<P>,
}

impl<F: PrimeField, P: Rounds> CRHGadget<F, P> {
	fn permute(
		parameters: &PoseidonParametersVar<F>,
		mut state: Vec<FpVar<F>>,
	) -> Result<Vec<FpVar<F>>, SynthesisError> {
		let width = P::WIDTH;

		let mut round_keys_offset = 0;

		// full Sbox rounds
		for _ in 0..(P::FULL_ROUNDS / 2) {
			// Substitution (S-box) layer
			for i in 0..width {
				state[i] += &parameters.round_keys[round_keys_offset];
				state[i] = P::SBOX.synthesize_sbox(&state[i])?;
				round_keys_offset += 1;
			}
			// Apply linear layer
			state = Self::apply_linear_layer(&state, &parameters.mds_matrix);
		}

		// middle partial Sbox rounds
		for _ in 0..P::PARTIAL_ROUNDS {
			// Substitution (S-box) layer
			for i in 0..width {
				state[i] += &parameters.round_keys[round_keys_offset];
				round_keys_offset += 1;
			}
			// apply Sbox to only 1 element of the state.
			// Here the last one is chosen but the choice is arbitrary.
			state[0] = P::SBOX.synthesize_sbox(&state[0])?;
			// Linear layer
			state = Self::apply_linear_layer(&state, &parameters.mds_matrix);
		}

		// last full Sbox rounds
		for _ in 0..(P::FULL_ROUNDS / 2) {
			// Substitution (S-box) layer
			for i in 0..width {
				state[i] += &parameters.round_keys[round_keys_offset];
				state[i] = P::SBOX.synthesize_sbox(&state[i])?;
				round_keys_offset += 1;
			}
			// Linear layer
			state = Self::apply_linear_layer(&state, &parameters.mds_matrix);
		}

		Ok(state)
	}

	fn apply_linear_layer(state: &Vec<FpVar<F>>, mds_matrix: &Vec<Vec<FpVar<F>>>) -> Vec<FpVar<F>> {
		let mut new_state: Vec<FpVar<F>> = Vec::new();
		for i in 0..state.len() {
			let mut sc = FpVar::<F>::zero();
			for j in 0..state.len() {
				let mij = &mds_matrix[i][j];
				sc += mij * &state[j];
			}
			new_state.push(sc);
		}
		new_state
	}
}

// https://github.com/arkworks-rs/r1cs-std/blob/master/src/bits/uint8.rs#L343
impl<F: PrimeField, P: Rounds> CRHGadgetTrait<CRH<F, P>, F> for CRHGadget<F, P> {
	type OutputVar = FpVar<F>;
	type ParametersVar = PoseidonParametersVar<F>;

	fn evaluate(
		parameters: &Self::ParametersVar,
		input: &[UInt8<F>],
	) -> Result<Self::OutputVar, SynthesisError> {
		let f_var_inputs: Vec<FpVar<F>> = to_field_var_elements(input)?;
		if f_var_inputs.len() > P::WIDTH {
			panic!(
				"incorrect input length {:?} for width {:?}",
				f_var_inputs.len(),
				P::WIDTH,
			);
		}

		let mut buffer = vec![FpVar::zero(); P::WIDTH];
		buffer
			.iter_mut()
			.zip(f_var_inputs)
			.for_each(|(b, l_b)| *b = l_b);

		let result = Self::permute(&parameters, buffer);
		result.map(|x| x.get(0).cloned().ok_or(SynthesisError::AssignmentMissing))?
	}
}

impl<F: PrimeField, P: Rounds> TwoToOneCRHGadget<CRH<F, P>, F> for CRHGadget<F, P> {
	type OutputVar = FpVar<F>;
	type ParametersVar = PoseidonParametersVar<F>;

	fn evaluate(
		parameters: &Self::ParametersVar,
		left_input: &[UInt8<F>],
		right_input: &[UInt8<F>],
	) -> Result<Self::OutputVar, SynthesisError> {
		// assume equality of left and right length
		assert_eq!(left_input.len(), right_input.len());
		let chained_input: Vec<_> = left_input
			.to_vec()
			.into_iter()
			.chain(right_input.to_vec().into_iter())
			.collect();
		<Self as CRHGadgetTrait<_, _>>::evaluate(parameters, &chained_input)
	}
}

impl<F: PrimeField> AllocVar<PoseidonParameters<F>, F> for PoseidonParametersVar<F> {
	fn new_variable<T: Borrow<PoseidonParameters<F>>>(
		_cs: impl Into<Namespace<F>>,
		f: impl FnOnce() -> Result<T, SynthesisError>,
		_mode: AllocationMode,
	) -> Result<Self, SynthesisError> {
		let params = f()?.borrow().clone();

		let mut round_keys_var = Vec::new();
		for rk in params.round_keys {
			round_keys_var.push(FpVar::Constant(rk));
		}
		let mut mds_var = Vec::new();
		for row in params.mds_matrix {
			let mut row_var = Vec::new();
			for mk in row {
				row_var.push(FpVar::Constant(mk));
			}
			mds_var.push(row_var);
		}
		Ok(Self {
			round_keys: round_keys_var,
			mds_matrix: mds_var,
		})
	}
}

#[cfg(test)]
mod test {
	use super::*;
	use ark_crypto_primitives::crh::CRH as CRHTrait;
	use ark_ed_on_bls12_381::Fq;
	use ark_ff::{to_bytes, Zero};
	use ark_relations::r1cs::ConstraintSystem;

	use crate::{
		poseidon::sbox::PoseidonSbox,
		utils::{get_mds_poseidon_bls381_x5_3, get_rounds_poseidon_bls381_x5_3},
	};

	#[derive(Default, Clone)]
	struct PoseidonRounds3;

	impl Rounds for PoseidonRounds3 {
		const FULL_ROUNDS: usize = 8;
		const PARTIAL_ROUNDS: usize = 57;
		const SBOX: PoseidonSbox = PoseidonSbox::Exponentiation(5);
		const WIDTH: usize = 3;
	}

	type PoseidonCRH3 = CRH<Fq, PoseidonRounds3>;
	type PoseidonCRH3Gadget = CRHGadget<Fq, PoseidonRounds3>;

	#[test]
	fn test_poseidon_native_equality() {
		let cs = ConstraintSystem::<Fq>::new_ref();

		let rounds = get_rounds_poseidon_bls381_x5_3::<Fq>();
		let mds = get_mds_poseidon_bls381_x5_3::<Fq>();

		let params = PoseidonParameters::<Fq>::new(rounds, mds);
		let params_var = PoseidonParametersVar::new_variable(
			cs.clone(),
			|| Ok(&params),
			AllocationMode::Constant,
		)
		.unwrap();

		// Test Poseidon on an input of 3 field elements. This will not require padding,
		// since the inputs are aligned to the expected input chunk size of 32.
		let aligned_inp = to_bytes![Fq::zero(), Fq::from(1u128), Fq::from(2u128)].unwrap();
		let aligned_inp_var =
			Vec::<UInt8<Fq>>::new_input(cs.clone(), || Ok(aligned_inp.clone())).unwrap();

		let res = PoseidonCRH3::evaluate(&params, &aligned_inp).unwrap();
		let res_var = <PoseidonCRH3Gadget as CRHGadgetTrait<_, _>>::evaluate(
			&params_var.clone(),
			&aligned_inp_var,
		)
		.unwrap();
		assert_eq!(res, res_var.value().unwrap());

		// Test Poseidon on an input of 6 bytes. This will require padding, since the
		// inputs are not aligned to the expected input chunk size of 32.
		let unaligned_inp: Vec<u8> = vec![1, 2, 3, 4, 5, 6];
		let unaligned_inp_var =
			Vec::<UInt8<Fq>>::new_input(cs.clone(), || Ok(unaligned_inp.clone())).unwrap();

		let res = PoseidonCRH3::evaluate(&params, &unaligned_inp).unwrap();
		let res_var = <PoseidonCRH3Gadget as CRHGadgetTrait<_, _>>::evaluate(
			&params_var.clone(),
			&unaligned_inp_var,
		)
		.unwrap();
		assert_eq!(res, res_var.value().unwrap());
	}
}
