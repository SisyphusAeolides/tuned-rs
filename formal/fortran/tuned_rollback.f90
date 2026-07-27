module tuned_rollback_model
    implicit none
    private

    integer, parameter :: max_entries = 16

    type, public :: rollback_state
        integer :: count = 0
        character(len=64) :: keys(max_entries) = ""
        logical :: pending(max_entries) = .false.
    end type rollback_state

    public :: record_original
    public :: restore_reverse
    public :: pending_count
    public :: state_is_valid

contains

    subroutine record_original(state, key)
        type(rollback_state), intent(inout) :: state
        character(len=*), intent(in) :: key
        integer :: index

        do index = 1, state%count
            if (trim(state%keys(index)) == trim(key)) return
        end do

        if (state%count >= max_entries) error stop "rollback journal capacity exceeded"
        state%count = state%count + 1
        state%keys(state%count) = key
        state%pending(state%count) = .true.
    end subroutine record_original

    subroutine restore_reverse(state, failing_key, observed, observed_count)
        type(rollback_state), intent(inout) :: state
        character(len=*), intent(in) :: failing_key
        character(len=64), intent(out) :: observed(max_entries)
        integer, intent(out) :: observed_count
        integer :: index

        observed = ""
        observed_count = 0
        do index = state%count, 1, -1
            if (.not. state%pending(index)) cycle
            observed_count = observed_count + 1
            observed(observed_count) = state%keys(index)
            if (trim(state%keys(index)) /= trim(failing_key)) then
                state%pending(index) = .false.
            end if
        end do
    end subroutine restore_reverse

    pure integer function pending_count(state) result(count)
        type(rollback_state), intent(in) :: state
        integer :: index

        count = 0
        do index = 1, state%count
            if (state%pending(index)) count = count + 1
        end do
    end function pending_count

    pure logical function state_is_valid(state) result(valid)
        type(rollback_state), intent(in) :: state
        integer :: left
        integer :: right

        valid = state%count >= 0 .and. state%count <= max_entries
        if (.not. valid) return

        do left = 1, state%count
            if (len_trim(state%keys(left)) == 0) then
                valid = .false.
                return
            end if
            do right = left + 1, state%count
                if (trim(state%keys(left)) == trim(state%keys(right))) then
                    valid = .false.
                    return
                end if
            end do
        end do
    end function state_is_valid

end module tuned_rollback_model

program tuned_rollback_check
    use tuned_rollback_model
    implicit none

    integer, parameter :: max_entries = 16
    type(rollback_state) :: state
    character(len=64) :: observed(max_entries)
    integer :: observed_count

    call record_original(state, "sysfs:first")
    call record_original(state, "sysfs:second")
    call record_original(state, "sysfs:third")
    call record_original(state, "sysfs:second")

    if (.not. state_is_valid(state)) error stop "invalid journal after record"
    if (state%count /= 3) error stop "duplicate journal key was recorded"

    call restore_reverse(state, "sysfs:second", observed, observed_count)
    if (observed_count /= 3) error stop "restore did not visit every pending entry"
    if (trim(observed(1)) /= "sysfs:third") error stop "restore order is not LIFO"
    if (trim(observed(2)) /= "sysfs:second") error stop "restore order lost middle entry"
    if (trim(observed(3)) /= "sysfs:first") error stop "restore order lost oldest entry"
    if (pending_count(state) /= 1) error stop "failed entry was not retained alone"

    call restore_reverse(state, "", observed, observed_count)
    if (observed_count /= 1) error stop "retry did not visit retained entry"
    if (trim(observed(1)) /= "sysfs:second") error stop "wrong retained entry"
    if (pending_count(state) /= 0) error stop "successful retry did not clear journal"
    if (.not. state_is_valid(state)) error stop "invalid journal after restore"
end program tuned_rollback_check
