{-# OPTIONS --safe #-}
module TunedRuntime where

open import Agda.Builtin.Bool using (Bool; true; false)
open import Agda.Builtin.Equality using (_≡_; refl)
open import Agda.Builtin.Nat using (Nat; _<_) 

and : Bool → Bool → Bool
and true right = right
and false _ = false

record RuntimeUnit : Set where
  constructor unit
  field
    priority : Nat
    ordinal : Nat
    enabled : Bool
    cpuinfoMatches : Bool
    unameMatches : Bool

open RuntimeUnit public

active : RuntimeUnit → Bool
active candidate =
  and (enabled candidate)
    (and (cpuinfoMatches candidate) (unameMatches candidate))

orderedBefore : RuntimeUnit → RuntimeUnit → Bool
orderedBefore left right with priority left < priority right
... | true = true
... | false with priority right < priority left
...   | true = false
...   | false = ordinal left < ordinal right

matchingEnabledUnit : RuntimeUnit
matchingEnabledUnit = unit 10 0 true true true

cpuinfoMismatch : RuntimeUnit
cpuinfoMismatch = unit 0 1 true false true

laterEqualPriority : RuntimeUnit
laterEqualPriority = unit 10 2 true true true

matchingUnitActivates : active matchingEnabledUnit ≡ true
matchingUnitActivates = refl

cpuinfoMismatchCannotActivate : active cpuinfoMismatch ≡ false
cpuinfoMismatchCannotActivate = refl

lowerPriorityRunsFirst :
  orderedBefore (unit 5 3 true true true) matchingEnabledUnit ≡ true
lowerPriorityRunsFirst = refl

equalPriorityKeepsDeclarationOrder :
  orderedBefore matchingEnabledUnit laterEqualPriority ≡ true
equalPriorityKeepsDeclarationOrder = refl
