module TunedVerification

%default total

public export
data Verdict
  = Match
  | Missing
  | Mismatch
  | Unsupported
  | ReadError

public export
issuePasses : (ignoreMissing : Bool) -> Verdict -> Bool
issuePasses _ Match = True
issuePasses True Missing = True
issuePasses False Missing = False
issuePasses _ Mismatch = False
issuePasses _ Unsupported = False
issuePasses _ ReadError = False

public export
reportPasses : (ignoreMissing : Bool) -> List Verdict -> Bool
reportPasses ignoreMissing [] = True
reportPasses ignoreMissing (verdict :: rest) =
  issuePasses ignoreMissing verdict && reportPasses ignoreMissing rest

public export
strictMissingFails : issuePasses False Missing = False
strictMissingFails = Refl

public export
ignoredMissingPasses : issuePasses True Missing = True
ignoredMissingPasses = Refl

public export
mismatchCannotBeWaived : issuePasses True Mismatch = False
mismatchCannotBeWaived = Refl

public export
unsupportedCannotBeWaived : issuePasses True Unsupported = False
unsupportedCannotBeWaived = Refl

public export
readErrorCannotBeWaived : issuePasses True ReadError = False
readErrorCannotBeWaived = Refl

public export
missingOnlyReportMayPass :
  reportPasses True [Match, Missing, Match] = True
missingOnlyReportMayPass = Refl

public export
mismatchReportAlwaysFails :
  reportPasses True [Match, Mismatch, Missing] = False
mismatchReportAlwaysFails = Refl
