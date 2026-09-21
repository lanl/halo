%undefine _debugsource_packages

Name:		halo
Version:	%{version}
Release:	1%{?dist}
Summary:	A cluster management system designed for Lustre and similar systems

License:	MIT
URL:		https://github.com/lanl/halo
Source0:	%{name}-%{version}.tar

BuildRequires: systemd-rpm-macros
BuildRequires: cargo
BuildRequires: capnproto

Requires: systemd

%description
HALO is a cluster management system designed for managing Lustre HA and similar
use cases. HALO was previously known as GoLustre, and only supported Lustre HA,
but now it can manage other cluster types.

LANL software release number: O4905

%prep
%autosetup -n %{name}-%{version}

%build
CARGO_PROFILE_RELEASE_DEBUG=true cargo build --release

%install
install -D -p -m 0755 target/release/halo %{buildroot}%{_sbindir}/halo
install -D -p -m 0755 target/release/halo_remote %{buildroot}%{_sbindir}/halo_remote
install -D -p -m 0755 target/release/halo_manager %{buildroot}%{_sbindir}/halo_manager

install -D -p -m 0644 systemd/halo.service %{buildroot}%{_unitdir}/halo.service
install -D -p -m 0644 systemd/halo-remote.service %{buildroot}%{_unitdir}/halo-remote.service

install -D -p -m 0644 docs/man/halo.1 %{buildroot}%{_mandir}/man1/halo.1
install -D -p -m 0644 docs/man/halo_remote.1 %{buildroot}%{_mandir}/man1/halo_remote.1
install -D -p -m 0644 docs/man/halo_manager.1 %{buildroot}%{_mandir}/man1/halo_manager.1

install -D -p -m 0644 sysconfig/halo %{buildroot}%{_sysconfdir}/sysconfig/halo
install -d -m 0755 %{buildroot}%{_sysconfdir}/halo

%files
%{_sbindir}/halo
%{_sbindir}/halo_remote
%{_sbindir}/halo_manager

%{_unitdir}/halo.service
%{_unitdir}/halo-remote.service

%{_mandir}/man1/halo.1*
%{_mandir}/man1/halo_remote.1*
%{_mandir}/man1/halo_manager.1*

%dir %{_sysconfdir}/halo
%config(noreplace) %{_sysconfdir}/sysconfig/halo

%post
%systemd_post halo.service halo-remote.service

%preun
%systemd_preun halo.service halo-remote.service

%postun
%systemd_postun_with_restart halo.service halo-remote.service

%changelog
