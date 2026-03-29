/**
 * @fileoverview TabNavigation Component - Tab navigation with overflow menu
 * @module TabNavigation
 */

import { useState, useRef, useEffect } from 'react';
import {
  DEFAULT_TABS,
} from '../constants/ui.js';

/**
 * Tab navigation with "More" dropdown for advanced tabs to prevent overflow
 */
function TabNavigation({
  tabs = DEFAULT_TABS,
  activeTab,
  onTabChange,
  className = ''
}) {
  const [moreOpen, setMoreOpen] = useState(false);
  const menuRef = useRef(null);

  // Close dropdown on outside click
  useEffect(() => {
    if (!moreOpen) return;
    const handleClick = (e) => {
      if (menuRef.current && !menuRef.current.contains(e.target)) {
        setMoreOpen(false);
      }
    };
    document.addEventListener('mousedown', handleClick);
    return () => document.removeEventListener('mousedown', handleClick);
  }, [moreOpen]);

  // Close dropdown on Escape
  useEffect(() => {
    if (!moreOpen) return;
    const handleKey = (e) => {
      if (e.key === 'Escape') setMoreOpen(false);
    };
    document.addEventListener('keydown', handleKey);
    return () => document.removeEventListener('keydown', handleKey);
  }, [moreOpen]);

  const handleTabClick = (tabId) => {
    onTabChange(tabId);
    setMoreOpen(false);
  };

  // Group tabs
  const mainTabs = tabs.filter(tab => tab.group === 'main');
  const advancedTabs = tabs.filter(tab => tab.group === 'advanced');

  const activeAdvancedTab = advancedTabs.find(tab => tab.id === activeTab);

  const renderTab = (tab) => {
    const isDisabled = tab.disabled || false;
    const isActive = activeTab === tab.id;

    return (
      <button
        key={tab.id}
        className={`tab ${isActive ? 'tab-active' : ''} ${isDisabled ? 'opacity-40 cursor-not-allowed' : ''}`}
        onClick={() => handleTabClick(tab.id)}
        disabled={isDisabled}
        aria-current={isActive ? 'page' : undefined}
        aria-label={`${tab.label} tab`}
      >
        {tab.label}
      </button>
    );
  };

  return (
    <nav className={`bg-surface border-b border-border px-10 flex items-center ${className}`}>
      {/* Main tabs */}
      <div className="flex">
        {mainTabs.map((tab) => renderTab(tab))}
      </div>

      {/* More dropdown for advanced tabs */}
      {advancedTabs.length > 0 && (
        <>
          <div className="w-px h-6 bg-border mx-4" />
          <div className="relative" ref={menuRef}>
            <button
              className={`tab flex items-center gap-1 ${activeAdvancedTab ? 'tab-active' : ''}`}
              onClick={() => setMoreOpen(prev => !prev)}
              aria-expanded={moreOpen}
              aria-haspopup="true"
              aria-label="More tabs"
            >
              {activeAdvancedTab ? activeAdvancedTab.label : 'More'}
              <svg
                className={`w-3 h-3 transition-transform ${moreOpen ? 'rotate-180' : ''}`}
                fill="none"
                stroke="currentColor"
                viewBox="0 0 24 24"
              >
                <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M19 9l-7 7-7-7" />
              </svg>
            </button>

            {moreOpen && (
              <div className="absolute top-full left-0 mt-px bg-surface border border-border rounded shadow-lg z-50 min-w-[180px] py-1">
                {advancedTabs.map((tab) => {
                  const isActive = activeTab === tab.id;
                  const isDisabled = tab.disabled || false;
                  return (
                    <button
                      key={tab.id}
                      className={`w-full text-left px-4 py-2 text-sm transition-colors
                        ${isActive ? 'text-gruvbox-bright bg-gruvbox-elevated font-medium' : 'text-secondary hover:text-gruvbox-bright hover:bg-gruvbox-elevated'}
                        ${isDisabled ? 'opacity-40 cursor-not-allowed' : 'cursor-pointer'}`}
                      onClick={() => handleTabClick(tab.id)}
                      disabled={isDisabled}
                      aria-current={isActive ? 'page' : undefined}
                    >
                      {tab.label}
                    </button>
                  );
                })}
              </div>
            )}
          </div>
        </>
      )}
    </nav>
  );
}

export default TabNavigation;
