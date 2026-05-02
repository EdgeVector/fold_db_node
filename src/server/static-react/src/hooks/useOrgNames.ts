import { useState, useEffect } from 'react'
import { orgClient } from '../api/clients/orgClient'

interface OrgEntry {
  org_hash: string
  org_name: string
}

/**
 * Hook that fetches org list and returns a map of org_hash → org_name.
 * Shared across components that need to display org context.
 */
export function useOrgNames(): Record<string, string> {
  const [orgNames, setOrgNames] = useState<Record<string, string>>({})

  useEffect(() => {
    orgClient
      .listOrgs()
      .then((res) => {
        // The API client returns { data: T }; some legacy callers passed the
        // raw payload directly, so we tolerate both shapes here.
        const payload: unknown = (res as { data?: unknown }).data ?? res
        const orgs: OrgEntry[] =
          payload && typeof payload === 'object' && 'orgs' in payload
            ? ((payload as { orgs?: OrgEntry[] }).orgs ?? [])
            : []
        const map: Record<string, string> = {}
        for (const org of orgs) map[org.org_hash] = org.org_name
        setOrgNames(map)
      })
      .catch(() => {})
  }, [])

  return orgNames
}
