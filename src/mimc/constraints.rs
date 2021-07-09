use super::{MiMCParameters, Rounds, CRH};
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

#[derive(Clone)]
pub struct MiMCParametersVar<F: PrimeField> {
	pub k: FpVar<F>,
	pub rounds: usize,
	pub num_inputs: usize,
	pub num_outputs: usize,
	pub round_keys: Vec<FpVar<F>>,
}

impl<F: PrimeField> Default for MiMCParametersVar<F> {
	fn default() -> Self {
		Self {
			k: FpVar::<F>::zero(),
			rounds: usize::default(),
			num_inputs: usize::default(),
			num_outputs: usize::default(),
			round_keys: Vec::default(),
		}
	}
}

pub struct CRHGadget<F: PrimeField, P: Rounds> {
	field: PhantomData<F>,
	params: PhantomData<P>,
}

impl<F: PrimeField, P: Rounds> CRHGadget<F, P> {
	fn mimc(
		parameters: &MiMCParametersVar<F>,
		state: Vec<FpVar<F>>,
	) -> Result<Vec<FpVar<F>>, SynthesisError> {
		assert!(state.len() == parameters.num_inputs);
		let mut l_out: FpVar<F> = FpVar::<F>::zero();
		let mut r_out: FpVar<F> = FpVar::<F>::zero();
		for i in 0..state.len() {
			let l: FpVar<F>;
			let r: FpVar<F>;
			if i == 0 {
				l = state[i].clone();
				r = FpVar::<F>::zero();
			} else {
				l = l_out.clone() + state[i].clone();
				r = r_out.clone();
			}

			let res = Self::feistel(parameters, l, r)?;
			l_out = res[0].clone();
			r_out = res[1].clone();
		}

		let mut outs = vec![];
		outs.push(l_out.clone());
		for _ in 0..parameters.num_outputs {
			let res = Self::feistel(parameters, l_out.clone(), r_out.clone())?;
			l_out = res[0].clone();
			r_out = res[1].clone();
			outs.push(l_out.clone());
		}

		Ok(outs)
	}

	fn feistel(
		parameters: &MiMCParametersVar<F>,
		left: FpVar<F>,
		right: FpVar<F>,
	) -> Result<[FpVar<F>; 2], SynthesisError> {
		let mut x_l = left.clone();
		let mut x_r = right.clone();
		let mut c: FpVar<F>;
		let mut t: FpVar<F>;
		let mut t2: FpVar<F>;
		let mut t4: FpVar<F>;
		for i in 0..parameters.rounds {
			c = if i == 0 || i == parameters.rounds - 1 {
				FpVar::<F>::zero()
			} else {
				parameters.round_keys[i - 1].clone()
			};
			t = if i == 0 {
				parameters.k.clone() + x_l.clone()
			} else {
				parameters.k.clone() + x_l.clone() + c
			};

			t2 = t.clone() * t.clone();
			t4 = t2.clone() * t2.clone();

			let temp_x_l = x_l.clone();
			let temp_x_r = x_r.clone();

			if i < parameters.rounds - 1 {
				x_l = if i == 0 { temp_x_r } else { temp_x_r + t4 * t };

				x_r = temp_x_l;
			} else {
				x_r = temp_x_r + t4 * t;
				x_l = temp_x_l;
			}
		}

		Ok([x_l, x_r])
	}
}

// https://github.com/arkworks-rs/r1cs-std/blob/master/src/bits/uint8.rs#L343
impl<F: PrimeField, P: Rounds> CRHGadgetTrait<CRH<F, P>, F> for CRHGadget<F, P> {
	type OutputVar = FpVar<F>;
	type ParametersVar = MiMCParametersVar<F>;

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

		let result = Self::mimc(&parameters, buffer);
		result.map(|x| x.get(0).cloned().ok_or(SynthesisError::AssignmentMissing))?
	}
}

impl<F: PrimeField, P: Rounds> TwoToOneCRHGadget<CRH<F, P>, F> for CRHGadget<F, P> {
	type OutputVar = FpVar<F>;
	type ParametersVar = MiMCParametersVar<F>;

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

impl<F: PrimeField> AllocVar<MiMCParameters<F>, F> for MiMCParametersVar<F> {
	fn new_variable<T: Borrow<MiMCParameters<F>>>(
		_cs: impl Into<Namespace<F>>,
		f: impl FnOnce() -> Result<T, SynthesisError>,
		_mode: AllocationMode,
	) -> Result<Self, SynthesisError> {
		let params = f()?.borrow().clone();

		let mut round_keys_var = Vec::new();
		for rk in params.round_keys {
			round_keys_var.push(FpVar::Constant(rk));
		}

		Ok(Self {
			round_keys: round_keys_var,
			k: FpVar::Constant(params.k),
			rounds: params.rounds,
			num_inputs: params.num_inputs,
			num_outputs: params.num_outputs,
		})
	}
}

#[cfg(test)]
mod test {
	use super::*;
	use ark_crypto_primitives::crh::CRH as CRHTrait;
	use ark_ed_on_bn254::Fq;
	use ark_ff::{to_bytes, Zero};
	use ark_relations::r1cs::ConstraintSystem;

	use crate::utils::mimc::ed_on_bn254_mimc::CONSTANTS;

	#[derive(Default, Clone)]
	struct MiMCRounds220;

	impl Rounds for MiMCRounds220 {
		const ROUNDS: usize = 220;
		const WIDTH: usize = 3;
	}

	type MiMC220 = CRH<Fq, MiMCRounds220>;
	type MiMC220Gadget = CRHGadget<Fq, MiMCRounds220>;

	#[test]
	fn test_mimc_native_equality() {
		let cs = ConstraintSystem::<Fq>::new_ref();

		let params = MiMCParameters::<Fq>::new(
			Fq::from(3),
			MiMCRounds220::ROUNDS,
			MiMCRounds220::WIDTH,
			MiMCRounds220::WIDTH,
			CONSTANTS.to_vec(),
		);

		let params_var =
			MiMCParametersVar::new_variable(cs.clone(), || Ok(&params), AllocationMode::Constant)
				.unwrap();

		// Test Poseidon on an input of 3 field elements. This will not require padding,
		// since the inputs are aligned to the expected input chunk size of 32.
		let aligned_inp = to_bytes![Fq::zero(), Fq::from(1u128), Fq::from(2u128)].unwrap();
		let aligned_inp_var =
			Vec::<UInt8<Fq>>::new_input(cs.clone(), || Ok(aligned_inp.clone())).unwrap();

		let res = MiMC220::evaluate(&params, &aligned_inp).unwrap();
		let res_var = <MiMC220Gadget as CRHGadgetTrait<_, _>>::evaluate(
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

		let res = MiMC220::evaluate(&params, &unaligned_inp).unwrap();
		let res_var = <MiMC220Gadget as CRHGadgetTrait<_, _>>::evaluate(
			&params_var.clone(),
			&unaligned_inp_var,
		)
		.unwrap();
		assert_eq!(res, res_var.value().unwrap());
	}
}
