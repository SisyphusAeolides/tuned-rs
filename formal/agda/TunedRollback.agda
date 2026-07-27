{-# OPTIONS --safe #-}
module TunedRollback where

open import Agda.Builtin.Equality using (_≡_; refl)
open import Agda.Builtin.List using (List; []; _∷_)
open import Agda.Builtin.Maybe using (Maybe; just; nothing)
open import Agda.Builtin.Nat using (Nat; zero; suc)

infixr 5 _++_

_++_ : {A : Set} → List A → List A → List A
[] ++ right = right
(item ∷ rest) ++ right = item ∷ (rest ++ right)

reverseAcc : {A : Set} → List A → List A → List A
reverseAcc [] output = output
reverseAcc (item ∷ rest) output = reverseAcc rest (item ∷ output)

reverse : {A : Set} → List A → List A
reverse input = reverseAcc input []

data Entry : Set where
  entry : Nat → Nat → Entry

record State : Set where
  constructor state
  field
    journal : List Entry

open State public

empty : State
empty = state []

recordOriginal : Entry → State → State
recordOriginal item (state entries) = state (entries ++ (item ∷ []))

restoreOrder : State → List Entry
restoreOrder (state entries) = reverse entries

first : Entry
first = entry zero (suc zero)

second : Entry
second = entry (suc zero) (suc (suc zero))

newestRestoresFirst :
  restoreOrder (recordOriginal second (recordOriginal first empty))
    ≡ second ∷ first ∷ []
newestRestoresFirst = refl

data Phase : Set where
  idle applying active rollingBack failed : Phase

data Event : Set where
  begin commit abort restored restoreFailed stop : Event

transition : Phase → Event → Maybe Phase
transition idle begin = just applying
transition applying commit = just active
transition applying abort = just rollingBack
transition active begin = just applying
transition active stop = just rollingBack
transition rollingBack restored = just idle
transition rollingBack restoreFailed = just failed
transition failed restored = just idle
transition _ _ = nothing

abortCannotActivate : transition applying abort ≡ just rollingBack
abortCannotActivate = refl

failedRestoreCannotActivate :
  transition rollingBack restoreFailed ≡ just failed
failedRestoreCannotActivate = refl

successfulRestoreReturnsIdle : transition rollingBack restored ≡ just idle
successfulRestoreReturnsIdle = refl
