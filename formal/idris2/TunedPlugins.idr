module TunedPlugins

%default total

public export
data Support = Unsupported | Supported

public export
data Scope = Global | DeviceScoped

public export
data Selection = AllDevices | SelectedDevices

public export
data OptionState = Invalid | Valid

public export
data Snapshot = Absent | Recorded

public export
data Transition = Reject | NoChange | Mutate

public export
validate : Support -> Scope -> Selection -> OptionState -> Snapshot -> Transition
validate Unsupported _ _ _ _ = Reject
validate Supported Global SelectedDevices _ _ = Reject
validate Supported _ _ Invalid _ = Reject
validate Supported _ _ Valid Absent = NoChange
validate Supported _ _ Valid Recorded = Mutate

public export
unsupportedPluginCannotMutate :
  validate Unsupported DeviceScoped SelectedDevices Valid Recorded = Reject
unsupportedPluginCannotMutate = Refl

public export
globalPluginRejectsDeviceSelector :
  validate Supported Global SelectedDevices Valid Recorded = Reject
globalPluginRejectsDeviceSelector = Refl

public export
invalidOptionCannotMutate :
  validate Supported DeviceScoped AllDevices Invalid Recorded = Reject
invalidOptionCannotMutate = Refl

public export
snapshotIsRequiredBeforeMutation :
  validate Supported DeviceScoped SelectedDevices Valid Absent = NoChange
snapshotIsRequiredBeforeMutation = Refl

public export
validTransactionalDeviceMutation :
  validate Supported DeviceScoped SelectedDevices Valid Recorded = Mutate
validTransactionalDeviceMutation = Refl

public export
validTransactionalGlobalMutation :
  validate Supported Global AllDevices Valid Recorded = Mutate
validTransactionalGlobalMutation = Refl
