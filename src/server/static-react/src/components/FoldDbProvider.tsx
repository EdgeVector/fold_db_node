import { Provider } from 'react-redux'
import { store as defaultStore } from '../store/store'
import type { ReactNode } from 'react'
import type { Store } from '@reduxjs/toolkit'

interface FoldDbProviderProps {
  children: ReactNode
  store?: Store
}

export const FoldDbProvider = ({ children, store }: FoldDbProviderProps) => {
  return <Provider store={store ?? defaultStore}>{children}</Provider>
}
