module TunedProfile

%default total

public export
data Phase
  = Idle
  | Applying
  | Active
  | RollingBack
  | Failed

public export
data Event
  = Begin
  | Commit
  | Abort
  | Restored
  | RestoreFailed
  | Stop

public export
transition : Phase -> Event -> Maybe Phase
transition Idle Begin = Just Applying
transition Applying Commit = Just Active
transition Applying Abort = Just RollingBack
transition Active Begin = Just Applying
transition Active Stop = Just RollingBack
transition RollingBack Restored = Just Idle
transition RollingBack RestoreFailed = Just Failed
transition Failed Restored = Just Idle
transition _ _ = Nothing

public export
data Reachable : Phase -> Phase -> Type where
  Here : Reachable phase phase
  Step : {event : Event} ->
         transition from event = Just middle ->
         Reachable middle final ->
         Reachable from final

public export
beginStartsApplication : transition Idle Begin = Just Applying
beginStartsApplication = Refl

public export
commitIsTheOnlyDeclaredActivation : transition Applying Commit = Just Active
commitIsTheOnlyDeclaredActivation = Refl

public export
abortRequiresRollback : transition Applying Abort = Just RollingBack
abortRequiresRollback = Refl

public export
failedRestoreCannotBecomeActive : transition RollingBack RestoreFailed = Just Failed
failedRestoreCannotBecomeActive = Refl

public export
successfulRestoreReturnsIdle : transition RollingBack Restored = Just Idle
successfulRestoreReturnsIdle = Refl

public export
failedStateCannotBeginDirectly : transition Failed Begin = Nothing
failedStateCannotBeginDirectly = Refl

public export
applyThenAbortThenRestore : Reachable Idle Idle
applyThenAbortThenRestore =
  Step {event = Begin} beginStartsApplication
    (Step {event = Abort} abortRequiresRollback
      (Step {event = Restored} successfulRestoreReturnsIdle Here))
