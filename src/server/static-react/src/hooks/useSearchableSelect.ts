/**
 * Custom hook for managing searchable select state and behavior.
 * Extracted from SelectField to reduce component complexity and improve reuse.
 */

import { useState, useCallback, type ChangeEvent } from 'react'
import {
  filterOptions,
  groupOptions,
  type SelectOption,
} from '../utils/selectFieldHelpers'

interface SearchableOption extends SelectOption {
  disabled?: boolean
}

export function useSearchableSelect(
  options: SearchableOption[] = [],
  onChange: (value: string) => void,
  autoClose = true,
) {
  const [searchTerm, setSearchTerm] = useState('')
  const [isOpen, setIsOpen] = useState(false)

  const filteredOptions = filterOptions(options, searchTerm)
  const groupedOptions = groupOptions(filteredOptions)

  const handleSearchChange = useCallback((e: ChangeEvent<HTMLInputElement>) => {
    setSearchTerm(e.target.value)
  }, [])

  const handleOptionSelect = useCallback(
    (option: SearchableOption) => {
      if (option.disabled) return

      onChange(option.value)

      if (autoClose) {
        setIsOpen(false)
        setSearchTerm('')
      }
    },
    [onChange, autoClose],
  )

  const openDropdown = useCallback(() => {
    setIsOpen(true)
  }, [])

  const closeDropdown = useCallback(() => {
    setIsOpen(false)
  }, [])

  const toggleDropdown = useCallback(() => {
    setIsOpen((prev) => !prev)
  }, [])

  const selectOption = useCallback(
    (optionValue: string) => {
      const option = options.find((opt) => opt.value === optionValue)
      if (option) {
        handleOptionSelect(option)
      }
    },
    [options, handleOptionSelect],
  )

  const clearSearch = useCallback(() => {
    setSearchTerm('')
  }, [])

  const state = {
    searchTerm,
    isOpen,
    filteredOptions,
    groupedOptions,
  }

  const actions = {
    setSearchTerm,
    openDropdown,
    closeDropdown,
    toggleDropdown,
    selectOption,
    clearSearch,
  }

  return {
    state,
    actions,
    handleSearchChange,
    handleOptionSelect,
  }
}

export default useSearchableSelect
