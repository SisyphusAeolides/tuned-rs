module TunedSysfs

%default total

public export
data Zone
  = InsideSysfs
  | OutsideSysfs

public export
data ControlState
  = Missing
  | Present

public export
data ApplyResult
  = Rejected
  | Applied

public export
data RollbackResult
  = Untouched
  | Restored

public export
applyControl : Zone -> ControlState -> Bool -> ApplyResult
applyControl InsideSysfs Present True = Applied
applyControl _ _ _ = Rejected

public export
rollbackControl : ApplyResult -> Bool -> RollbackResult
rollbackControl Applied True = Restored
rollbackControl _ _ = Untouched

public export
outsidePathIsRejected :
  applyControl OutsideSysfs Present True = Rejected
outsidePathIsRejected = Refl

public export
missingControlIsRejected :
  applyControl InsideSysfs Missing True = Rejected
missingControlIsRejected = Refl

public export
unreadableControlIsRejected :
  applyControl InsideSysfs Present False = Rejected
unreadableControlIsRejected = Refl

public export
validControlIsApplied :
  applyControl InsideSysfs Present True = Applied
validControlIsApplied = Refl

public export
recordedApplyCanBeRestored :
  rollbackControl Applied True = Restored
recordedApplyCanBeRestored = Refl

public export
missingSnapshotCannotInventRestore :
  rollbackControl Applied False = Untouched
missingSnapshotCannotInventRestore = Refl
