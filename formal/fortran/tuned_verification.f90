module tuned_verification_model
    implicit none

    integer, parameter :: verdict_match = 0
    integer, parameter :: verdict_missing = 1
    integer, parameter :: verdict_mismatch = 2
    integer, parameter :: verdict_unsupported = 3
    integer, parameter :: verdict_read_error = 4

contains

    pure integer function classify_target(present, readable, supported, equal_value) result(verdict)
        logical, intent(in) :: present
        logical, intent(in) :: readable
        logical, intent(in) :: supported
        logical, intent(in) :: equal_value

        if (.not. supported) then
            verdict = verdict_unsupported
        else if (.not. present) then
            verdict = verdict_missing
        else if (.not. readable) then
            verdict = verdict_read_error
        else if (.not. equal_value) then
            verdict = verdict_mismatch
        else
            verdict = verdict_match
        end if
    end function classify_target

    pure logical function issue_passes(verdict, ignore_missing) result(passes)
        integer, intent(in) :: verdict
        logical, intent(in) :: ignore_missing

        select case (verdict)
        case (verdict_match)
            passes = .true.
        case (verdict_missing)
            passes = ignore_missing
        case default
            passes = .false.
        end select
    end function issue_passes

    pure logical function report_passes(verdicts, ignore_missing) result(passes)
        integer, intent(in) :: verdicts(:)
        logical, intent(in) :: ignore_missing
        integer :: index

        passes = .true.
        do index = 1, size(verdicts)
            if (.not. issue_passes(verdicts(index), ignore_missing)) then
                passes = .false.
                return
            end if
        end do
    end function report_passes

end module tuned_verification_model

program tuned_verification_check
    use tuned_verification_model
    implicit none

    integer :: verdicts(4)

    if (classify_target(.true., .true., .true., .true.) /= verdict_match) then
        error stop "matching target was not accepted"
    end if
    if (classify_target(.false., .true., .true., .true.) /= verdict_missing) then
        error stop "missing target was not classified"
    end if
    if (classify_target(.true., .true., .true., .false.) /= verdict_mismatch) then
        error stop "mismatch was not classified"
    end if
    if (classify_target(.true., .true., .false., .true.) /= verdict_unsupported) then
        error stop "unsupported target was not classified"
    end if
    if (classify_target(.true., .false., .true., .true.) /= verdict_read_error) then
        error stop "read failure was not classified"
    end if

    verdicts = [verdict_match, verdict_missing, verdict_match, verdict_match]
    if (report_passes(verdicts, .false.)) then
        error stop "strict verification ignored missing hardware"
    end if
    if (.not. report_passes(verdicts, .true.)) then
        error stop "ignore-missing rejected a missing-only report"
    end if

    verdicts = [verdict_match, verdict_mismatch, verdict_match, verdict_match]
    if (report_passes(verdicts, .false.) .or. report_passes(verdicts, .true.)) then
        error stop "a mismatch was incorrectly waivable"
    end if

    verdicts = [verdict_match, verdict_unsupported, verdict_match, verdict_match]
    if (report_passes(verdicts, .false.) .or. report_passes(verdicts, .true.)) then
        error stop "an unsupported operation was incorrectly waivable"
    end if

    verdicts = [verdict_match, verdict_read_error, verdict_match, verdict_match]
    if (report_passes(verdicts, .false.) .or. report_passes(verdicts, .true.)) then
        error stop "a read failure was incorrectly waivable"
    end if
end program tuned_verification_check
