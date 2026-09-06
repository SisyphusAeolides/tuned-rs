{-# OPTIONS --safe #-}
module TunedPlugins where

open import Agda.Builtin.Bool using (Bool; true; false)
open import Agda.Builtin.Equality using (_≡_; refl)

data Support : Set where
  unsupported supported : Support

data Scope : Set where
  global deviceScoped : Scope

data Selection : Set where
  allDevices selectedDevices : Selection

data OptionState : Set where
  invalid valid : OptionState

data Snapshot : Set where
  absent recorded : Snapshot

data Transition : Set where
  reject noChange mutate : Transition

data ResourceState : Set where
  closed active : ResourceState

validate : Support → Scope → Selection → OptionState → Snapshot → Transition
validate unsupported _ _ _ _ = reject
validate supported global selectedDevices _ _ = reject
validate supported _ _ invalid _ = reject
validate supported _ _ valid absent = noChange
validate supported _ _ valid recorded = mutate

unsupportedPluginCannotMutate :
  validate unsupported deviceScoped selectedDevices valid recorded ≡ reject
unsupportedPluginCannotMutate = refl

globalPluginRejectsDeviceSelector :
  validate supported global selectedDevices valid recorded ≡ reject
globalPluginRejectsDeviceSelector = refl

invalidOptionCannotMutate :
  validate supported deviceScoped allDevices invalid recorded ≡ reject
invalidOptionCannotMutate = refl

snapshotIsRequiredBeforeMutation :
  validate supported deviceScoped selectedDevices valid absent ≡ noChange
snapshotIsRequiredBeforeMutation = refl

validTransactionalDeviceMutation :
  validate supported deviceScoped selectedDevices valid recorded ≡ mutate
validTransactionalDeviceMutation = refl

validTransactionalGlobalMutation :
  validate supported global allDevices valid recorded ≡ mutate
validTransactionalGlobalMutation = refl

acquireResource : Support → ResourceState → ResourceState
acquireResource supported _ = active
acquireResource unsupported state = state

releaseResource : ResourceState → ResourceState
releaseResource _ = closed

resourceAcquireIsIdempotent :
  acquireResource supported (acquireResource supported closed) ≡ active
resourceAcquireIsIdempotent = refl

rollbackReleasesRuntimeResource :
  releaseResource (acquireResource supported closed) ≡ closed
rollbackReleasesRuntimeResource = refl

unsupportedResourceStaysClosed :
  acquireResource unsupported closed ≡ closed
unsupportedResourceStaysClosed = refl
