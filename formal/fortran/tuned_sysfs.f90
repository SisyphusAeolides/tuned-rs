module tuned_sysfs_model
    implicit none

    integer, parameter :: rejected = 0
    integer, parameter :: applied = 1
    integer, parameter :: restored = 2

contains

    pure integer function apply_control(inside_sysfs, control_exists, readable) result(state)
        logical, intent(in) :: inside_sysfs
        logical, intent(in) :: control_exists
        logical, intent(in) :: readable

        if (inside_sysfs .and. control_exists .and. readable) then
            state = applied
        else
            state = rejected
        end if
    end function apply_control

    pure integer function rollback_control(state, snapshot_recorded) result(next_state)
        integer, intent(in) :: state
        logical, intent(in) :: snapshot_recorded

        if (state == applied .and. snapshot_recorded) then
            next_state = restored
        else
            next_state = state
        end if
    end function rollback_control

end module tuned_sysfs_model

program tuned_sysfs_check
    use tuned_sysfs_model
    implicit none

    if (apply_control(.false., .true., .true.) /= rejected) then
        error stop "path outside sysfs was accepted"
    end if
    if (apply_control(.true., .false., .true.) /= rejected) then
        error stop "missing sysfs control was accepted"
    end if
    if (apply_control(.true., .true., .false.) /= rejected) then
        error stop "unreadable sysfs control was accepted"
    end if
    if (apply_control(.true., .true., .true.) /= applied) then
        error stop "valid sysfs control was rejected"
    end if
    if (rollback_control(applied, .true.) /= restored) then
        error stop "recorded sysfs control was not restored"
    end if
    if (rollback_control(applied, .false.) /= applied) then
        error stop "rollback invented a missing snapshot"
    end if
end program tuned_sysfs_check
