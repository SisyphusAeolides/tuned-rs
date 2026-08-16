{-# OPTIONS --safe #-}
module TunedUnits where

open import Agda.Builtin.Bool using (Bool; true; false)
open import Agda.Builtin.Equality using (_≡_; refl)
open import Agda.Builtin.Nat using (Nat)

data OptionState : Set where
  absent : OptionState
  present : Nat → OptionState

overlayOption : Bool → Bool → OptionState → OptionState → OptionState
overlayOption true _ _ incoming = incoming
overlayOption false true _ _ = absent
overlayOption false false base absent = base
overlayOption false false _ incoming@(present _) = incoming

incomingOverrides :
  overlayOption false false (present 10) (present 20) ≡ present 20
incomingOverrides = refl

unspecifiedPreservesBase :
  overlayOption false false (present 10) absent ≡ present 10
unspecifiedPreservesBase = refl

dropRemovesInherited :
  overlayOption false true (present 10) (present 20) ≡ absent
dropRemovesInherited = refl

replaceDiscardsInherited :
  overlayOption true false (present 10) absent ≡ absent
replaceDiscardsInherited = refl

replaceTakesPrecedenceOverDropMetadata :
  overlayOption true true (present 10) (present 20) ≡ present 20
replaceTakesPrecedenceOverDropMetadata = refl
