// Interfaces from apps/gnome/dbus/*.xml, embedded because the installed extension cannot
// reach the repository; test.js fails when they drift from those files.

export const APP_NAME = 'com.dictto.App';
export const APP_PATH = '/com/dictto/App';
export const SHELL_NAME = 'com.dictto.Shell';
export const SHELL_PATH = '/com/dictto/Shell';

export const APP_XML = `<node>
  <interface name="com.dictto.App1">
    <method name="ShowSettings"/>
    <method name="Quit"/>
    <method name="BarStart"/>
    <method name="BarStop"/>
    <method name="BarCancel"/>
    <method name="SetHotkeyCapture">
      <arg name="active" type="b" direction="in"/>
    </method>
    <method name="SetSetting">
      <arg name="key" type="s" direction="in"/>
      <arg name="value" type="s" direction="in"/>
      <arg name="error" type="s" direction="out"/>
    </method>
    <method name="GetSettings">
      <arg name="settings" type="a{ss}" direction="out"/>
    </method>
    <method name="SaveKey">
      <arg name="key" type="s" direction="in"/>
      <arg name="error" type="s" direction="out"/>
    </method>
    <method name="RemoveKey">
      <arg name="error" type="s" direction="out"/>
    </method>
    <method name="KeyHint">
      <arg name="hint" type="s" direction="out"/>
    </method>
    <method name="Microphones">
      <arg name="microphones" type="a(sssb)" direction="out"/>
    </method>
    <method name="DiscoverSpeechModels">
      <arg name="generation" type="t" direction="in"/>
    </method>
    <signal name="Event">
      <arg name="name" type="s"/>
      <arg name="payload_json" type="s"/>
    </signal>
    <property name="Status" type="s" access="read"/>
  </interface>
</node>`;

export const SHELL_XML = `<node>
  <interface name="com.dictto.Shell1">
    <method name="SaveClipboard">
      <arg name="token" type="u" direction="out"/>
    </method>
    <method name="RestoreClipboard">
      <arg name="token" type="u" direction="in"/>
    </method>
    <method name="SetText">
      <arg name="text" type="s" direction="in"/>
    </method>
    <method name="GetText">
      <arg name="which" type="s" direction="in"/>
      <arg name="present" type="b" direction="out"/>
      <arg name="text" type="s" direction="out"/>
    </method>
  </interface>
</node>`;
