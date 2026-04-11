"use client";

import { useAtomValue, useSetAtom } from "jotai/react";
import { activePortAtom, sessionsAtom, newSessionDialogAtom } from "@/store/sessions";
import { useSessionsSync } from "@/store/sessions";
import { useStreamSync, hasConsoleErrorsAtom, consoleLogsAtom } from "@/store/stream";
import { useActivitySync } from "@/store/activity";
import { activeExtensionsAtom } from "@/store/sessions";
import { useChatStatusSync } from "@/store/chat";
import { useMediaQuery } from "@/hooks/use-media-query";
import { Viewport } from "@/components/viewport";
import { ActivityFeed } from "@/components/activity-feed";
import { ChatPanel } from "@/components/chat-panel";
import { ConsolePanel } from "@/components/console-panel";
import { StoragePanel } from "@/components/storage-panel";
import { ExtensionsPanel } from "@/components/extensions-panel";
import { NetworkPanel } from "@/components/network-panel";
import { SessionTree } from "@/components/session-tree";
import {
  ResizablePanelGroup,
  ResizablePanel,
  ResizableHandle,
} from "@/components/ui/resizable";
import { Tabs, TabsList, TabsTrigger, TabsContent } from "@/components/ui/tabs";
import { Button } from "@/components/ui/button";
import { Plus } from "lucide-react";

export default function DashboardPage() {
  const activePort = useAtomValue(activePortAtom);
  useStreamSync(activePort);
  useSessionsSync();
  useActivitySync();
  useChatStatusSync();

  const sessions = useAtomValue(sessionsAtom);
  const hasSessions = sessions.length > 0;
  const setNewSessionDialog = useSetAtom(newSessionDialogAtom);
  const isDesktop = useMediaQuery("(min-width: 768px)");
  const hasConsoleErrors = useAtomValue(hasConsoleErrorsAtom);
  const activeExtensions = useAtomValue(activeExtensionsAtom);

  const sidePanel = (
    <Tabs defaultValue="chat" className="flex h-full flex-col">
      <div className="shrink-0 px-2 pt-1">
        <TabsList variant="line" className="h-7 w-full">
          <TabsTrigger value="chat" className="text-[11px]">Chat</TabsTrigger>
          <TabsTrigger value="activity" className="text-[11px]">Activity</TabsTrigger>
          <TabsTrigger value="console" className="text-[11px]">
            Console
            {hasConsoleErrors && (
              <span className="ml-1 inline-flex size-1.5 rounded-full bg-destructive" />
            )}
          </TabsTrigger>
          <TabsTrigger value="network" className="text-[11px]">Network</TabsTrigger>
          <TabsTrigger value="storage" className="text-[11px]">Storage</TabsTrigger>
          <TabsTrigger value="extensions" className="text-[11px]">
            Extensions
            {activeExtensions.length > 0 && (
              <span className="ml-1 text-[9px] tabular-nums text-muted-foreground">{activeExtensions.length}</span>
            )}
          </TabsTrigger>
        </TabsList>
      </div>
      <TabsContent value="activity" className="min-h-0 flex-1 overflow-hidden">
        <ActivityFeed />
      </TabsContent>
      <TabsContent value="console" className="min-h-0 flex-1 overflow-hidden">
        <ConsolePanel />
      </TabsContent>
      <TabsContent value="network" className="min-h-0 flex-1 overflow-hidden">
        <NetworkPanel />
      </TabsContent>
      <TabsContent value="storage" className="min-h-0 flex-1 overflow-hidden">
        <StoragePanel />
      </TabsContent>
      <TabsContent value="extensions" className="min-h-0 flex-1 overflow-hidden">
        <ExtensionsPanel />
      </TabsContent>
      <TabsContent value="chat" className="min-h-0 flex-1 overflow-hidden">
        <ChatPanel />
      </TabsContent>
    </Tabs>
  );

  if (isDesktop) {
    if (!hasSessions) {
      return (
        <div className="flex h-screen flex-col" style={{ background: 'var(--surface-200)' }}>
          <ResizablePanelGroup
            orientation="horizontal"
            className="min-h-0 flex-1"
          >
            <ResizablePanel id="sessions" defaultSize="15%" minSize="10%" maxSize="30%">
              <SessionTree />
            </ResizablePanel>
            <ResizableHandle />
            <ResizablePanel id="empty" defaultSize="85%">
              <div className="flex h-full items-center justify-center">
                <div className="text-center" style={{ maxWidth: 320 }}>
                  {/* Warm decorative icon */}
                  <div className="mx-auto mb-8 flex items-center justify-center w-20 h-20 rounded-2xl" style={{ background: 'var(--surface-400)', border: '1px solid rgba(38,37,30,0.1)' }}>
                    <svg width="36" height="36" viewBox="0 0 36 36" fill="none" xmlns="http://www.w3.org/2000/svg">
                      <rect x="4" y="8" width="28" height="20" rx="3" stroke="rgba(38,37,30,0.4)" strokeWidth="1.5" fill="none"/>
                      <path d="M4 13h28" stroke="rgba(38,37,30,0.4)" strokeWidth="1.5"/>
                      <circle cx="8" cy="10.5" r="1.2" fill="rgba(38,37,30,0.4)"/>
                      <circle cx="12" cy="10.5" r="1.2" fill="rgba(38,37,30,0.25)"/>
                      <circle cx="16" cy="10.5" r="1.2" fill="rgba(38,37,30,0.15)"/>
                      <rect x="8" y="17" width="12" height="2" rx="1" fill="rgba(38,37,30,0.2)"/>
                      <rect x="8" y="22" width="20" height="2" rx="1" fill="rgba(38,37,30,0.12)"/>
                    </svg>
                  </div>

                  <h2 className="mb-3 text-xl font-semibold" style={{ color: 'var(--cursor-dark)', letterSpacing: '-0.02em' }}>
                    No active sessions
                  </h2>
                  <p className="mb-8 text-sm leading-relaxed" style={{ color: 'rgba(38,37,30,0.55)' }}>
                    Create a session to start browsing with AI assistance
                  </p>

                  <button
                    onClick={() => setNewSessionDialog(true)}
                    className="inline-flex items-center gap-2 px-5 py-2.5 rounded-lg text-sm font-medium transition-all cursor-pointer"
                    style={{
                      background: 'var(--surface-400)',
                      color: 'var(--cursor-dark)',
                      border: '1px solid rgba(38,37,30,0.1)',
                      letterSpacing: '0.01em',
                    }}
                    onMouseEnter={e => e.currentTarget.style.color = 'var(--cursor-error)'}
                    onMouseLeave={e => e.currentTarget.style.color = 'var(--cursor-dark)'}
                  >
                    <Plus size={16} />
                    New session
                  </button>
                </div>
              </div>
            </ResizablePanel>
          </ResizablePanelGroup>
        </div>
      );
    }

    return (
      <div className="flex h-screen flex-col" style={{ background: 'var(--surface-200)' }}>
        <ResizablePanelGroup
          orientation="horizontal"
          className="min-h-0 flex-1"
        >
          <ResizablePanel id="sessions" defaultSize="15%" minSize="10%" maxSize="30%">
            <SessionTree />
          </ResizablePanel>
          <ResizableHandle />
          <ResizablePanel id="viewport" defaultSize="55%" minSize="30%">
            <Viewport />
          </ResizablePanel>
          <ResizableHandle />
          <ResizablePanel id="activity" defaultSize="30%" minSize="15%" maxSize="50%">
            {sidePanel}
          </ResizablePanel>
        </ResizablePanelGroup>
      </div>
    );
  }

  return (
    <div className="flex h-screen flex-col" style={{ background: 'var(--surface-200)' }}>
      <Tabs defaultValue="viewport" className="min-h-0 flex-1">
        <div className="shrink-0 px-2 pt-2">
          <TabsList className="w-full">
            <TabsTrigger value="sessions">Sessions</TabsTrigger>
            <TabsTrigger value="viewport">Viewport</TabsTrigger>
            <TabsTrigger value="activity">Activity</TabsTrigger>
          </TabsList>
        </div>
        <TabsContent value="sessions" className="min-h-0 overflow-hidden">
          <SessionTree />
        </TabsContent>
        <TabsContent value="viewport" className="min-h-0 overflow-hidden">
          <Viewport />
        </TabsContent>
        <TabsContent value="activity" className="min-h-0 overflow-hidden">
          {sidePanel}
        </TabsContent>
      </Tabs>
    </div>
  );
}
