import React from 'react'
import { screen, fireEvent, waitFor } from '@testing-library/react'
import { describe, it, expect, beforeEach, vi } from 'vitest'
import SchemaTab from '../../../components/tabs/SchemaTab'
import { renderWithRedux, createTestSchemaState } from '../../utils/testUtilities.jsx'

// Mock schemaClient
vi.mock('../../../api/clients/schemaClient', () => ({
  default: {
    getSchema: vi.fn(() => Promise.resolve({
      success: true,
      data: { name: 'test-schema', state: 'approved', fields: {} }
    })),
    listSchemaKeys: vi.fn(() => Promise.resolve({
      success: true,
      data: { keys: [], total_count: 0 }
    }))
  }
}))

// Mock Redux actions
vi.mock('../../../store/schemaSlice', async () => {
  const actual = await vi.importActual('../../../store/schemaSlice')

  const createMockAction = (actionType) => vi.fn(() => {
    // Return a thunk function that returns a Promise
    const thunk = () => {
      const action = {
        type: actionType,
        payload: undefined,
        meta: {
          requestId: 'test-id',
          requestStatus: 'fulfilled'
        }
      }
      return Promise.resolve(action)
    }

    // Add fulfilled property to the thunk so it matches what createAsyncThunk returns
    thunk.fulfilled = { match: () => true }
    return thunk
  })

  return {
    ...actual,
    approveSchema: createMockAction('schemas/approveSchema/fulfilled'),
    blockSchema: createMockAction('schemas/blockSchema/fulfilled'),
    fetchSchemas: createMockAction('schemas/fetchSchemas/fulfilled')
  }
})

describe('SchemaTab Component', () => {
  const mockProps = {
    schemas: [], // This prop is not used by the current component
    onResult: vi.fn(),
    onSchemaUpdated: vi.fn()
  }

  beforeEach(() => {
    vi.clearAllMocks()
  })

  it('renders approved schemas section', async () => {
    await renderWithRedux(<SchemaTab {...mockProps} />, {
      preloadedState: createTestSchemaState()
    })

    await waitFor(() => {
      expect(screen.getByText('No schemas match the current filters.')).toBeInTheDocument()
    })
  })

  it('displays local schemas and hides available (global) schemas', async () => {
    const schemaState = createTestSchemaState({
      schemas: {
        'schema1': { name: 'Schema1', state: 'Approved', fields: [] },
        'schema2': { name: 'Schema2', state: 'Blocked', fields: [] },
        'schema3': { name: 'Schema3', state: 'Available', fields: [] }
      }
    })

    await renderWithRedux(<SchemaTab {...mockProps} />, {
      preloadedState: schemaState
    })

    // Should display only local schemas (approved/blocked), not available (global catalog)
    await waitFor(() => {
      expect(screen.getByText('Schema1')).toBeInTheDocument()
      expect(screen.getByText('Schema2')).toBeInTheDocument()
    })
    expect(screen.queryByText('Schema3')).not.toBeInTheDocument()
  })

  it('displays no approved schemas message when empty', async () => {
    await renderWithRedux(<SchemaTab {...mockProps} />, {
      preloadedState: createTestSchemaState()
    })

    await waitFor(() => {
      expect(screen.getByText('No schemas match the current filters.')).toBeInTheDocument()
    })
  })

  it('shows a row-action menu with Block for approved schemas', async () => {
    // Block was an inline button per row; with 50+ schemas that's 50+ red
    // buttons stacked. Now it lives behind a "⋯" menu (aria-label "More
    // actions for ..."). Open the menu, assert "Block schema" is there.
    const schemaState = createTestSchemaState({
      schemas: {
        'approvedSchema': { name: 'ApprovedSchema', state: 'Approved', fields: [] }
      }
    })

    await renderWithRedux(<SchemaTab {...mockProps} />, {
      preloadedState: schemaState
    })

    await waitFor(() => {
      expect(
        screen.getByRole('button', { name: /more actions for ApprovedSchema/i })
      ).toBeInTheDocument()
    })

    fireEvent.click(
      screen.getByRole('button', { name: /more actions for ApprovedSchema/i })
    )

    expect(
      screen.getByRole('menuitem', { name: /block schema/i })
    ).toBeInTheDocument()
  })

  it('handles schema blocking via the row-action menu', async () => {
    const { blockSchema } = await import('../../../store/schemaSlice')

    const schemaState = createTestSchemaState({
      schemas: {
        'approvedSchema': { name: 'ApprovedSchema', state: 'Approved', fields: [] }
      }
    })

    await renderWithRedux(<SchemaTab {...mockProps} />, {
      preloadedState: schemaState
    })

    // Open the row's ⋯ menu, then click "Block schema".
    await waitFor(() => {
      fireEvent.click(
        screen.getByRole('button', { name: /more actions for ApprovedSchema/i })
      )
    })

    fireEvent.click(screen.getByRole('menuitem', { name: /block schema/i }))

    await waitFor(() => {
      expect(blockSchema).toHaveBeenCalledWith({ schemaName: 'ApprovedSchema' })
    })
  })

  it('fetches and displays fields when expanding an approved schema', async () => {
    const schemaState = createTestSchemaState({
      schemas: {
        'approvedSchema': {
          name: 'ApprovedSchema',
          state: 'Approved',
          schema_type: { Single: {} },
          fields: ['id', 'name', 'email']
        }
      }
    })

    await renderWithRedux(<SchemaTab {...mockProps} />, {
      preloadedState: schemaState
    })

    // Expand the approved schema to display fields
    await waitFor(() => {
      fireEvent.click(screen.getByText('ApprovedSchema'))
    })

    // Verify fields are displayed
    await waitFor(() => {
      expect(screen.getByText('id')).toBeInTheDocument()
      expect(screen.getByText('name')).toBeInTheDocument()
      expect(screen.getByText('email')).toBeInTheDocument()
    })
  })

  it('groups schemas into System and User sections and hides empty system schemas by default', async () => {
    const schemaState = createTestSchemaState({
      schemas: {
        identity: { name: 'identity', state: 'Approved', fields: [] },
        persona: { name: 'persona', state: 'Approved', fields: [] },
        user_profiles: { name: 'user_profiles', state: 'Approved', fields: [] },
      },
    })

    await renderWithRedux(<SchemaTab {...mockProps} />, { preloadedState: schemaState })

    await waitFor(() => {
      expect(screen.getByText('User schemas')).toBeInTheDocument()
      expect(screen.getByText('System schemas')).toBeInTheDocument()
    })

    // User schema is always visible.
    expect(screen.getByText('user_profiles')).toBeInTheDocument()
    // System section is collapsed by default when nothing has data, so system
    // schemas are not rendered.
    expect(screen.queryByText('identity')).not.toBeInTheDocument()
    expect(screen.queryByText('persona')).not.toBeInTheDocument()

    // Clicking "Show" expands the section.
    fireEvent.click(screen.getByText('Show'))
    await waitFor(() => {
      expect(screen.getByText('identity')).toBeInTheDocument()
      expect(screen.getByText('persona')).toBeInTheDocument()
    })
  })

  it('trusts the backend-provided `system: false` flag even for known system names', async () => {
    // If the backend ever reclassifies a name in the known-system allow-list,
    // the flag must win. Here `identity` is explicitly system=false.
    const schemaState = createTestSchemaState({
      schemas: {
        identity: { name: 'identity', state: 'Approved', fields: [], system: false },
      },
    })

    await renderWithRedux(<SchemaTab {...mockProps} />, { preloadedState: schemaState })

    await waitFor(() => {
      expect(screen.getByText('User schemas')).toBeInTheDocument()
      expect(screen.getByText('identity')).toBeInTheDocument()
    })
    expect(screen.queryByText('System schemas')).not.toBeInTheDocument()
  })
})