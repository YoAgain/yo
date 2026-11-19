import SwiftUI

extension View {
  /// The system's confirmation dialog while `item` is set: a destructive `action` and `cancel`; `run` gets the item.
  func confirmDestructive<Item>(
    _ item: Binding<Item?>, title: @escaping (Item) -> String, message: ((Item) -> String)? = nil,
    action: @escaping (Item) -> String, cancel: String = "Cancel", run: @escaping (Item) -> Void
  ) -> some View {
    confirmationDialog(
      item.wrappedValue.map(title) ?? "",
      isPresented: item.isPresent,
      titleVisibility: .visible, presenting: item.wrappedValue
    ) { value in
      Button(action(value), role: .destructive) { run(value) }
      Button(cancel, role: .cancel) {}
    } message: { value in
      if let text = message?(value), !text.isEmpty { Text(text) }
    }
  }

  /// An OK alert while `problem` is set, with it as the message.
  func problemAlert(_ problem: Binding<String?>) -> some View { problemAlert("Something Went Wrong", problem) }

  func problemAlert(_ title: String, _ problem: Binding<String?>) -> some View {
    alert(title, isPresented: problem.isPresent) {
      Button("OK", role: .cancel) {}
    } message: {
      Text(problem.wrappedValue ?? "")
    }
  }
}

extension Optional {
  /// Set while there is a value; set to false, it clears it (`$item.isPresent` for `isPresented:`).
  var isPresent: Bool {
    get { self != nil }
    set { if !newValue { self = nil } }
  }
}
