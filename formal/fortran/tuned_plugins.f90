module tuned_plugins_model
    implicit none

    integer, parameter :: transition_reject = 0
    integer, parameter :: transition_no_change = 1
    integer, parameter :: transition_mutate = 2
    integer, parameter :: resource_closed = 0
    integer, parameter :: resource_open = 1

contains

    pure integer function validate_plugin(plugin_supported, device_scoped, selects_devices, &
            option_valid, snapshot_recorded) result(transition)
        logical, intent(in) :: plugin_supported
        logical, intent(in) :: device_scoped
        logical, intent(in) :: selects_devices
        logical, intent(in) :: option_valid
        logical, intent(in) :: snapshot_recorded

        if (.not. plugin_supported) then
            transition = transition_reject
        else if (selects_devices .and. .not. device_scoped) then
            transition = transition_reject
        else if (.not. option_valid) then
            transition = transition_reject
        else if (.not. snapshot_recorded) then
            transition = transition_no_change
        else
            transition = transition_mutate
        end if
    end function validate_plugin

    pure integer function acquire_resource(plugin_supported, state) result(next_state)
        logical, intent(in) :: plugin_supported
        integer, intent(in) :: state

        if (plugin_supported) then
            next_state = resource_open
        else
            next_state = state
        end if
    end function acquire_resource

    pure integer function release_resource(state) result(next_state)
        integer, intent(in) :: state

        if (state == resource_open) then
            next_state = resource_closed
        else
            next_state = resource_closed
        end if
    end function release_resource

end module tuned_plugins_model

program tuned_plugins_check
    use tuned_plugins_model
    implicit none

    if (validate_plugin(.false., .true., .true., .true., .true.) /= transition_reject) then
        error stop "unsupported plugin was allowed to mutate"
    end if
    if (validate_plugin(.true., .false., .true., .true., .true.) /= transition_reject) then
        error stop "global plugin accepted a device selector"
    end if
    if (validate_plugin(.true., .true., .false., .false., .true.) /= transition_reject) then
        error stop "invalid plugin option was allowed to mutate"
    end if
    if (validate_plugin(.true., .true., .true., .true., .false.) /= transition_no_change) then
        error stop "mutation occurred without a rollback snapshot"
    end if
    if (validate_plugin(.true., .true., .true., .true., .true.) /= transition_mutate) then
        error stop "valid transactional device mutation was rejected"
    end if
    if (validate_plugin(.true., .false., .false., .true., .true.) /= transition_mutate) then
        error stop "valid transactional global mutation was rejected"
    end if
    if (acquire_resource(.true., acquire_resource(.true., resource_closed)) /= resource_open) then
        error stop "runtime resource acquisition is not idempotent"
    end if
    if (release_resource(acquire_resource(.true., resource_closed)) /= resource_closed) then
        error stop "rollback did not release runtime resource"
    end if
    if (acquire_resource(.false., resource_closed) /= resource_closed) then
        error stop "unsupported runtime resource was acquired"
    end if
end program tuned_plugins_check
