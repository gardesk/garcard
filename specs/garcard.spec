Name:           garcard
Version:        0.1.0
Release:        1%{?dist}
Summary:        Polkit authentication agent for the gar desktop suite

License:        MIT
URL:            https://github.com/gardesk/garcard
Source0:        %{name}-%{version}.tar.gz

BuildRequires:  rust >= 1.75
BuildRequires:  cargo
BuildRequires:  dbus-devel

# Disable debug package
%global debug_package %{nil}

%description
garcard is a user-session Polkit authentication agent for X11 built in Rust.
Provides Gardesk-native authentication prompts for privileged desktop actions,
plus daemon diagnostics via garcardctl.

%prep
# Pre-built from monorepo

%build
# Pre-built from monorepo

%install
mkdir -p %{buildroot}%{_bindir}
install -Dm755 %{_sourcedir}/garcard %{buildroot}%{_bindir}/garcard
install -Dm755 %{_sourcedir}/garcardctl %{buildroot}%{_bindir}/garcardctl

%files
%{_bindir}/garcard
%{_bindir}/garcardctl

%changelog
* Tue Feb 17 2026 mfw <espadonne@outlook.com> - 0.1.0-1
- Initial RPM release of garcard
- Polkit authentication agent for gardesk desktop suite
