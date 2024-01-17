%define __spec_install_post %{nil}
%define __os_install_post %{_dbpath}/brp-compress
%define debug_package %{nil}

Name: sl-hello-goodbye
Summary: Tails chat.txt log files and processes local chat lines and Entered chat range notifications to produce desktop notifications for people to still say Hello too and similar for goodbyes
Version: @@VERSION@@
Release: @@RELEASE@@
License: MIT
Group: Applications/System
Source0: %{name}-%{version}.tar.gz

BuildRoot: %{_tmppath}/%{name}-%{version}-%{release}-root

%description
%{summary}

%prep
%setup -q

%install
rm -rf %{buildroot}
mkdir -p %{buildroot}
cp -a * %{buildroot}

%clean
rm -rf %{buildroot}

%files
%defattr(-,root,root,-)
%{_bindir}/*
